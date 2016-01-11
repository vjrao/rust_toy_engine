//! Internal data representation for the entity component system.
use core::nonzero::NonZero;
use memory::Vector;
use std::ptr;

use super::{
    Component, 
    COMPONENT_ALIGN, 
    Entity,
    
    SMALL_SIZE,
    MEDIUM_SIZE,
    LARGE_SIZE,
};
use super::entity::index_of;
use super::world::WorldAllocator;

const INITIAL_CAPACITY: usize = 512;

#[derive(Clone, Copy)]
// What granularity an entity's data is.
enum Granularity {
    Small,
    Medium,
    Large, 
}

// Metadata about a slab.
struct SlabMetadata {
    block_tracker: Vector<bool, WorldAllocator>,
    offset: usize,
}

impl SlabMetadata {
    fn resize_to(&mut self, size: usize) {
        while self.block_tracker.len() < size {
            self.block_tracker.push(false);
        }
    }
    
    // gets the index of the next free block and marks it used.
    fn next_block(&mut self) -> Option<usize> {
        for (idx, flag) in self.block_tracker.iter_mut().enumerate() {
            if !*flag {
                *flag = true;
                return Some(idx);
            }
        }
        
        None
    }
    
    // Acquire a known-free block. Panics if not actually free or out-of-bounds.
    fn acquire_block(&mut self, idx: usize) {
        let flag = self.block_tracker.get_mut(idx).expect("Attempted to acquire out-of-bounds block");
        if *flag {
            panic!("Attempted to acquire used block");
        } else {
            *flag = true;
        }
    }
    
    // Mark a block to be free.
    unsafe fn mark_free(&mut self, idx: usize) {
        self.block_tracker[idx] = false;
    }
    
    // whether the block at index `idx` is alive.
    // panics on out-of-bounds.
    fn is_alive(&self, idx: usize) -> bool {
        self.block_tracker[idx]
    }
}

const PADDING_SIZE: usize = COMPONENT_ALIGN - 8;

// A block's header.
#[repr(C)]
struct BlockHeader {
    entity: Entity,
    freelist_begin: u32,
    padding: [u8; PADDING_SIZE], // to reach component alignment.
}

// given a raw block size, compute the size of the block with a header added.
fn block_size_with_header(block_size: usize) -> usize {
    let size = ::std::mem::size_of::<BlockHeader>();
    debug_assert!(size == COMPONENT_ALIGN);
    
    // maybe change block header in the future to compute an offset to where
    // block data actually begins, to be more accommodating of different 
    // component alignments.
    size + block_size
}

const NO_OFFSET: u32 = ::std::u32::MAX;

fn round_up_to(n: usize, m: usize) -> usize {
    (n + m - 1) / m * m
}

#[repr(C)]
struct EmptySlot {
    size: u32,
    next_free: u32,
    padding: [u8; PADDING_SIZE]
}

struct BlockHandle {
    header: NonZero<*mut BlockHeader>,
    data: NonZero<*mut u8>,
    granularity: Granularity,
}

impl BlockHandle {
    // create a block handle from a pointer to the beginning of the header.
    unsafe fn from_raw(ptr: *mut u8, granularity: Granularity) -> Self {
        assert!(!ptr.is_null());
        let data = ptr.offset(::std::mem::size_of::<BlockHeader>() as isize);
        
        BlockHandle {
            header: NonZero::new(ptr as *mut BlockHeader),
            data: NonZero::new(data),
            granularity: granularity
        }
    }
    
    // mark the block as entirely empty.
    // this does not erase the entity stored in the header.
    // this can only be done when recycling a block.
    unsafe fn clear(&mut self) {
        let slot = EmptySlot {
            size: match self.granularity {
                Granularity::Small => SMALL_SIZE,
                Granularity::Medium => MEDIUM_SIZE,
                Granularity::Large => LARGE_SIZE,
            } as u32,
            next_free: NO_OFFSET,
            padding: [0; PADDING_SIZE],
        };
        
        ptr::write(*self.data as *mut EmptySlot, slot);
        (**self.header).freelist_begin = 0;
    }
    
    // marks the slot at the given offset from the start of the data to free.
    unsafe fn mark_free(&mut self, offset: usize, size: usize) {
        use std::u32;
        
        assert!(offset < u32::MAX as usize);
        assert!(size < u32::MAX as usize);
        
        let mut prev_off = None;
        let mut next_off = (**self.header).freelist_begin;
        let this_slot = self.data.offset(offset as isize) as *mut EmptySlot;
        
        let size = round_up_to(size, COMPONENT_ALIGN) as u32;
        
        // list is totally empty.
        if next_off == NO_OFFSET {
            (**self.header).freelist_begin = offset as u32;
            ptr::write(this_slot, EmptySlot {
                size: size as u32,
                next_free: NO_OFFSET,
                padding: [0; PADDING_SIZE],
            });
            
            return;
        }
        
        while next_off != NO_OFFSET {
            debug_assert!(next_off != offset as u32); // better not be pushing a slot on twice.
            // we order slots in the freelist by their offset in the block. this makes
            // merging slots very simple.
            if next_off > offset as u32 {
                if let Some(prev_slot) = prev_off.map(|o| self.data.offset(o as isize) as *mut EmptySlot) {
                    ptr::write(this_slot, EmptySlot {
                        size: size,
                        next_free: (*prev_slot).next_free,
                        padding: [0; PADDING_SIZE],
                    });
                    
                    (*prev_slot).next_free = offset as u32;
                } else {
                    // special case: inserting into front of free list.
                    ptr::write(this_slot, EmptySlot {
                        size: size,
                        next_free: (**self.header).freelist_begin,
                        padding: [0; PADDING_SIZE],
                    });
                    
                    (**self.header).freelist_begin = offset as u32;
                }
                
                return;
            } else {
                let next_slot = self.data.offset(next_off as isize) as *mut EmptySlot;
                prev_off = Some(next_off);
                next_off = (*next_slot).next_free;
            }
        }
        
        // put onto the end of list.
        let last_slot = self.data.offset(next_off as isize) as *mut EmptySlot;
        (*last_slot).next_free = offset as u32;
        
        ptr::write(this_slot, EmptySlot {
           size: size,
           next_free: NO_OFFSET,
           padding: [0; PADDING_SIZE], 
        });
    }
    
    // find the first free slot with the given size, rounding up to the next multiple
    // of COMPONENT_ALIGN to reduce fragmentation.
    // returns None if there is no room.
    unsafe fn next_free(&mut self, size: usize) -> Option<usize> {
        let mut prev_off = None;
        let mut next_off = (**self.header).freelist_begin;
        let size = round_up_to(size, COMPONENT_ALIGN) as u32;
        
        while next_off != NO_OFFSET {
            let this_off = next_off;
            let cur_slot = self.data.offset(this_off as isize) as *mut EmptySlot;
            let next_slot = (*cur_slot).next_free;
            let slot_size = (*cur_slot).size;
            
            if slot_size == size {
                // found an exact fit.              
                if this_off == (**self.header).freelist_begin {
                    // update the freelist start if this is the first slot in the list.
                    (**self.header).freelist_begin = next_slot;
                } else if let Some(prev) = prev_off {
                    // otherwise, if there's a previous slot (should be), take this entry from the list.
                    let prev_slot = self.data.offset(prev as isize) as *mut EmptySlot;
                    (*prev_slot).next_free = next_slot;
                }
                
                return Some(this_off as usize);
            } else if slot_size > size {
                // found a slot we can split.
                let (split_off, split_size) = (this_off + size, slot_size - size);
                assert!(split_size as usize % COMPONENT_ALIGN == 0, "Split slot has invalid size.");
                
                if this_off == (**self.header).freelist_begin {
                    (**self.header).freelist_begin = split_off;
                } else if let Some(prev) = prev_off {
                    let prev_slot = self.data.offset(prev as isize) as *mut EmptySlot;
                    (*prev_slot).next_free = split_off;
                }
                
                let split_slot = self.data.offset(split_off as isize) as *mut EmptySlot;
                ptr::write(split_slot, EmptySlot {
                    size: split_size,
                    next_free: next_slot,
                    padding: [0; PADDING_SIZE],
                });
                
                return Some(this_off as usize);
            } else {
                // keep looping, we'll find it eventually.
                prev_off = Some(this_off);
                next_off = next_slot;
            }
        }
        
        None
    }
    
    // utility function for merging adjacent slots.
    // slot are said to be adjacent if their offset plus their size
    // adds up to the offset of the next block.
    unsafe fn merge_adjacent_slots(&mut self) {
        let mut prev_off = None;
        let mut next_off = (**self.header).freelist_begin;
        
        while next_off != NO_OFFSET {
            let cur_slot = self.data.offset(next_off as isize) as *mut EmptySlot;
            let next_slot = (*cur_slot).next_free;
            next_off = next_slot;
            
            // test if the two blocks are adjacent, merge if they are, and update
            // the loop vars to match.
            if let Some(prev) = prev_off {
                let prev_slot = self.data.offset(prev as isize) as *mut EmptySlot;
                let prev_size = (*prev_slot).size;
                if prev + prev_size as u32 == next_off {
                    // blocks are adjacent. combine their sizes and don't 
                    // update the prev_off loop var.
                    let new_size = prev_size + (*cur_slot).size;
                    ptr::write(prev_slot, EmptySlot {
                        size: new_size,
                        next_free: next_slot,
                        padding: [0; PADDING_SIZE],
                    });

                    continue;
                }
            }
            
            // blocks are not adjacent. continue looping as normal.
            prev_off = Some(next_off);
        }
    }
}

// The data blob is where all the component data is stored.
// It is a contiguous block of memory logically separated into "slabs" of the granularities above.
// Each granularity is given the same number of possible entries.
struct Blob {
    data: Option<*mut u8>,
    data_kind: Option<::memory::Kind>,
    blocks_per_slab: usize,
    alloc: WorldAllocator,
    
    small: SlabMetadata,
    medium: SlabMetadata,
    large: SlabMetadata,
}

impl Blob {
    // Create a new blob. This does not allocate.    
    fn new(alloc: WorldAllocator) -> Self {
        Blob {
            data: None,
            data_kind: None,
            blocks_per_slab: 0,
            alloc: alloc,
            
            small: SlabMetadata {
                block_tracker: Vector::with_alloc(alloc),
                offset: 0,
            },
            medium: SlabMetadata {
                block_tracker: Vector::with_alloc(alloc),
                offset: 0,
            },
            large: SlabMetadata {
                block_tracker: Vector::with_alloc(alloc),
                offset: 0,
            }
        }
    }
    
    // get the offset of the "small" block.
    #[inline]
    fn small_offset(&self) -> usize {
        0
    }
    
    // get the offset of the "medium" block.
    #[inline]
    fn medium_offset(&self) -> usize {
        self.medium.offset
    }
    
    // get the offset of the "large" block.
    #[inline]
    fn large_offset(&self) -> usize {
        self.large.offset
    }
    
    // Returns the first free block in the given granularity's slab.
    // grows the buffer if necessary.
    fn next_block(&mut self, granularity: Granularity) -> BlockHandle {
        // scan the given granularity's bookkeeping table to find the first free block.
        let maybe_index = match granularity {
            Granularity::Small => self.small.next_block(),
            Granularity::Medium => self.medium.next_block(),
            Granularity::Large => self.large.next_block(),
        };
        
        let index = maybe_index.unwrap_or_else(|| {
            // out of room. grow and grab the first newly-allocated block.
            let old_num_blocks = self.blocks_per_slab;
            self.grow();
            
            match granularity {
                Granularity::Small => self.small.acquire_block(old_num_blocks),
                Granularity::Medium => self.medium.acquire_block(old_num_blocks),
                Granularity::Large => self.large.acquire_block(old_num_blocks),
            }
            
            old_num_blocks
        });
        
        debug_assert!(match granularity {
            Granularity::Small => !self.small.is_alive(index),
            Granularity::Medium => !self.medium.is_alive(index),
            Granularity::Large => !self.large.is_alive(index),
        });
        
        unsafe {
            let (slab_start, block_size) = match granularity {
                Granularity::Small => (self.small_offset(), block_size_with_header(SMALL_SIZE)),
                Granularity::Medium => (self.medium_offset(), block_size_with_header(MEDIUM_SIZE)),
                Granularity::Large => (self.large_offset(), block_size_with_header(LARGE_SIZE)),
            };
    
            // at this point, we've already either gotten a free block or grown the buffer if there 
            // weren't any. The data pointer is available.
            let mut block_handle = BlockHandle::from_raw(self.data.unwrap()
                .offset((slab_start + (index * block_size)) as isize), granularity);
                
            block_handle.clear();
            block_handle
        }
    }
    
    // doubles the size of each slab.
    fn grow(&mut self) {
        use core::nonzero::NonZero;
        use memory::{Allocator, Kind};
        
        let new_size = if self.blocks_per_slab == 0 { 
            INITIAL_CAPACITY
        } else {
            self.blocks_per_slab * 2
        };
        
        // size of each slab.
        let small_size = new_size * block_size_with_header(SMALL_SIZE);
        let medium_size = new_size * block_size_with_header(MEDIUM_SIZE);
        let large_size = new_size * block_size_with_header(LARGE_SIZE);
        
        // slab allocation layouts.
        // make sure they're all aligned.
        unsafe {
            let small_kind = Kind::array::<u8>(small_size)
                .map(|kind| kind.align_to(NonZero::new(COMPONENT_ALIGN))).expect("capacity overflow");
                
            let medium_kind = Kind::array::<u8>(medium_size)
                .map(|kind| kind.align_to(NonZero::new(COMPONENT_ALIGN))).expect("capacity overflow");
                
            let large_kind = Kind::array::<u8>(large_size)
                .map(|kind| kind.align_to(NonZero::new(COMPONENT_ALIGN))).expect("capacity overflow");
                
            // combine the allocations into one, accounting for any padding.
            let (small_med, med_off) = small_kind.extend(medium_kind).expect("capacity overflow");
            let (all_slabs, large_off) = small_med.extend(large_kind).expect("capacity overflow");
        
            debug_assert!(*all_slabs.align() >= COMPONENT_ALIGN);
        
            // perform the allocation, just diverge if out of memory.
            let alloc_res = if let Some(ptr) = self.data.take() {
                self.alloc.realloc(NonZero::new(ptr), self.data_kind.take().unwrap(), all_slabs)   
            } else {
                self.alloc.alloc(all_slabs)
            };
            
            if alloc_res.is_err() {
                self.alloc.oom()
            }
            
            self.data = Some(*alloc_res.unwrap());
            self.data_kind = Some(all_slabs);
        
            // update slab metadata
            self.medium.offset = med_off;
            self.large.offset = large_off;
            
            self.small.resize_to(new_size);
            self.medium.resize_to(new_size);
            self.large.resize_to(new_size);
            
            self.blocks_per_slab = new_size;
        }
    }
    
    // get a handle to live block.
    fn get_block(&self, granularity: Granularity, index: usize) -> Option<BlockHandle> {
        debug_assert!(match granularity {
            Granularity::Small => self.small.is_alive(index),
            Granularity::Medium => self.medium.is_alive(index),
            Granularity::Large => self.large.is_alive(index),
        });
        
        let off = match granularity {
            Granularity::Small => self.small_offset() + index * block_size_with_header(SMALL_SIZE),
            Granularity::Medium => self.medium_offset() + index * block_size_with_header(MEDIUM_SIZE),
            Granularity::Large => self.large_offset() + index * block_size_with_header(LARGE_SIZE)
        };
        
        self.data.clone().map(|data_ptr| unsafe {
            BlockHandle::from_raw(data_ptr.offset(off as isize), granularity)
        })
    }
    
    // Free a block. It must not be used again until it is next allocated.
    unsafe fn free_block(&mut self, granularity: Granularity, index: usize) {
        debug_assert!(match granularity {
            Granularity::Small => self.small.is_alive(index),
            Granularity::Medium => self.medium.is_alive(index),
            Granularity::Large => self.large.is_alive(index),
        });
                     
        match granularity {
            Granularity::Small => self.small.mark_free(index),
            Granularity::Medium => self.medium.mark_free(index),
            Granularity::Large => self.large.mark_free(index),
        }
    }
}

impl Drop for Blob {
    fn drop(&mut self) {
        use core::nonzero::NonZero;
        use memory::Allocator;
        
        unsafe {
            if let Some(kind) = self.data_kind.take() {
                let _ = self.alloc.dealloc(NonZero::new(self.data.unwrap()), kind);
            }
        }
    }
}

// Stores a granularity, offset pair for each entity.
struct MasterOffsetTable {
    data: Vector<Option<(Granularity, usize)>, WorldAllocator>,
}

impl MasterOffsetTable {
    fn set(&mut self, entity: Entity, granularity: Granularity, offset: usize) {
        let idx = index_of(entity) as usize;
        self.ensure_capacity(idx);
        self.data[idx] = Some((granularity, offset));
    }
    
    fn get(&self, entity: Entity) -> Option<(Granularity, usize)> {
        let entry: Option<&Option<(Granularity, usize)>> = self.data.get(index_of(entity) as usize);
        entry.map(Clone::clone).unwrap_or(None)
    }
    
    fn remove(&mut self, entity: Entity) {
        let entry: Option<&mut Option<(Granularity, usize)>> = self.data.get_mut(index_of(entity) as usize);
        if let Some(entry) = entry {
            *entry = None;
        }
    }
    
    // ensure enough capacity for `size` elements.
    fn ensure_capacity(&mut self, size: usize) {
        while self.data.len() < size {
            self.data.push(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Blob, Granularity};
    use ecs::world::WorldAllocator;
    use memory::DefaultAllocator;
    
    #[test]
    fn grow_blob() {
        let alloc = WorldAllocator(DefaultAllocator);
        let mut blob = Blob::new(alloc);
        for _ in 0..8 {
            blob.grow();
        }
    }
}