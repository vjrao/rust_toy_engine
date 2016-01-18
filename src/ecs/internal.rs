//! Internal data representation for the entity component system.
use core::nonzero::NonZero;
use memory::Vector;

use std::cmp::Ordering;
use std::marker::PhantomData;
use std::ptr;

use super::{Component, COMPONENT_ALIGN, Entity, SMALL_SIZE, MEDIUM_SIZE, LARGE_SIZE};
use super::entity::index_of;
use super::world::WorldAllocator;

const INITIAL_CAPACITY: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd)]
// What granularity an entity's data is.
pub enum Granularity {
    Small,
    Medium,
    Large,
}

impl Granularity {
    fn size(&self) -> usize {
        match *self {
            Granularity::Small => SMALL_SIZE,
            Granularity::Medium => MEDIUM_SIZE,
            Granularity::Large => LARGE_SIZE,
        }
    }
}

// Data related to a specific slab..
struct Slab {
    // list of free blocks, in reverse-sorted order.
    block_tracker: Vector<usize, WorldAllocator>,
    data: *mut u8,
    size: usize,
    block_size: usize,
    alloc: WorldAllocator,
}

impl Slab {
    fn new(alloc: WorldAllocator, block_size: usize) -> Self {
        Slab {
            block_tracker: Vector::with_alloc_and_capacity(alloc, INITIAL_CAPACITY),
            data: ptr::null_mut(),
            size: 0,
            block_size: block_size,
            alloc: alloc,
        }
    }

    // grows this slab.
    fn grow(&mut self) {
        use memory::Allocator;
        
        let new_size = if self.data.is_null() { INITIAL_CAPACITY } else  { self.size * 2 };
        let kind = self.make_kind(new_size).expect("capacity overflow");
            
        let new_data = if self.data.is_null() {
            unsafe {
                match self.alloc.alloc(kind) {
                    Ok(ptr) => *ptr,
                    Err(_) => self.alloc.oom(),
                }
            }
        } else {
            // can unwrap this because the new kind is larger than this and succeeded.
            let old_kind = self.make_kind(new_size).unwrap();                
            unsafe {
                match self.alloc.realloc(NonZero::new(self.data), old_kind, kind) {
                    Ok(ptr) => *ptr,
                    Err(_) => self.alloc.oom(),
                } 
            }
        };
        
        for idx in (self.size..new_size).rev() {
            self.block_tracker.push(idx);
        }
        
        self.size = new_size;
        self.data = new_data;
    }

    // gets the index of the next free block and marks it used.
    // grows if necessary.
    fn next_block(&mut self) -> usize {
        if let Some(b) = self.block_tracker.pop() {
            b
        } else {
            self.grow();
            self.block_tracker.pop().unwrap()
        }
    }

    // Mark a block to be free.
    unsafe fn mark_free(&mut self, idx: usize) {
        // fast path for fairly common case.
        let push = match self.block_tracker.last() {
            Some(last) if *last > idx => true,
            None => true,
            _ => false,
        };
        
        if push {
            self.block_tracker.push(idx);
            return;
        }
        
        // we shouldn't have found this block, since it isn't supposed to be in the freelist.
        if let Err(insertion_point) = self.search_tracker_for(idx) {
            self.block_tracker.insert(insertion_point, idx);
        }
    }
    
    // Gets a pointer to the block at the given offset,
    // making no guarantees over whether it is dead or alive.
    // panics if offset is out of bounds.
    unsafe fn get_block(&self, idx: usize) -> *mut u8 {
        assert!(idx < self.size);
        
        let block_size = block_size_with_header(self.block_size);
        self.data.offset((idx * block_size) as isize)
    }

    // whether the block at index `idx` is alive.
    fn is_alive(&self, idx: usize) -> bool {
        idx < self.size && self.search_tracker_for(idx).is_err()
    }
    
    // creates an allocation request suitable for the given number of blocks.
    fn make_kind(&self, size: usize) -> Option<::memory::Kind> {
        use memory::Kind;
        
        let sized = Kind::array::<u8>(size * block_size_with_header(self.block_size));
        unsafe { sized.map(|k| k.align_to(NonZero::new(COMPONENT_ALIGN))) }
    }

    #[inline]
    fn search_tracker_for(&self, idx: usize) -> Result<usize, usize> {
        self.block_tracker.binary_search_by(|probe| probe.cmp(&idx).reverse())
    }
}

impl Drop for Slab {
    fn drop(&mut self) {
        use memory::Allocator;
        
        if !self.data.is_null() {
            if let Some(kind) = self.make_kind(self.size) {
                unsafe {
                    let _ = self.alloc.dealloc(NonZero::new(self.data), kind);
                }
            }
        }
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
    padding: [u8; PADDING_SIZE],
}

#[derive(Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct Offset {
    granularity: Granularity,
    inner_off: usize,
}

impl Offset {
    pub fn new(granularity: Granularity, offset: usize) -> Self {
        Offset {
            granularity: granularity,
            inner_off: offset,
        }
    }
    
    pub fn into_parts(self) -> (Granularity, usize) {
        (self.granularity, self.inner_off)
    }
}

impl Ord for Offset {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.granularity as usize).cmp(&(other.granularity as usize)) {
            Ordering::Equal => self.inner_off.cmp(&other.inner_off),
            other => other,
        }
    }
}

pub enum SlotError {
    TooBig, // there is not enough room even with a promotion to allocate this component.
    NeedsPromote(Granularity), // needs a promotion to the contained granularity.
}

pub struct BlockHandle {
    header: NonZero<*mut BlockHeader>,
    data: NonZero<*mut u8>,
    granularity: Granularity,
    index: usize,
}

impl BlockHandle {
    // create a block handle from a pointer to the beginning of the header.
    unsafe fn from_raw(ptr: *mut u8, granularity: Granularity, index: usize) -> Self {
        assert!(!ptr.is_null());
        let data = ptr.offset(::std::mem::size_of::<BlockHeader>() as isize);

        BlockHandle {
            header: NonZero::new(ptr as *mut BlockHeader),
            data: NonZero::new(data),
            granularity: granularity,
            index: index,
        }
    }

    // mark the block as entirely empty.
    // this does not erase the entity stored in the header.
    // this can only be done when recycling a block.
    unsafe fn clear(&mut self) {
        let slot = EmptySlot {
            size: self.granularity.size() as u32,
            next_free: NO_OFFSET,
            padding: [0; PADDING_SIZE],
        };

        ptr::write(*self.data as *mut EmptySlot, slot);
        (**self.header).freelist_begin = 0;
    }

    // get a pointer to the start of the data.
    pub unsafe fn data_ptr(&self) -> *mut u8 {
        *self.data
    }

    // marks the slot at the given offset from the start of the data to free.
    pub unsafe fn mark_free(&mut self, offset: usize, size: usize) {
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
            ptr::write(this_slot,
                       EmptySlot {
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
                if let Some(prev_slot) = prev_off.map(|o| {
                    self.data.offset(o as isize) as *mut EmptySlot
                }) {
                    ptr::write(this_slot,
                               EmptySlot {
                                   size: size,
                                   next_free: (*prev_slot).next_free,
                                   padding: [0; PADDING_SIZE],
                               });

                    (*prev_slot).next_free = offset as u32;
                } else {
                    // special case: inserting into front of free list.
                    ptr::write(this_slot,
                               EmptySlot {
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

        ptr::write(this_slot,
                   EmptySlot {
                       size: size,
                       next_free: NO_OFFSET,
                       padding: [0; PADDING_SIZE],
                   });
    }

    // find the first free slot with the given size, rounding up to the next multiple
    // of COMPONENT_ALIGN to reduce fragmentation.
    pub unsafe fn next_free(&mut self, size: usize) -> Result<usize, SlotError> {
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

                return Ok(this_off as usize);
            } else if slot_size > size {
                // found a slot we can split.
                let (split_off, split_size) = (this_off + size, slot_size - size);
                assert!(split_size as usize % COMPONENT_ALIGN == 0,
                        "Split slot has invalid size.");

                if this_off == (**self.header).freelist_begin {
                    (**self.header).freelist_begin = split_off;
                } else if let Some(prev) = prev_off {
                    let prev_slot = self.data.offset(prev as isize) as *mut EmptySlot;
                    (*prev_slot).next_free = split_off;
                }

                let split_slot = self.data.offset(split_off as isize) as *mut EmptySlot;
                ptr::write(split_slot,
                           EmptySlot {
                               size: split_size,
                               next_free: next_slot,
                               padding: [0; PADDING_SIZE],
                           });

                return Ok(this_off as usize);
            } else {
                prev_off = Some(this_off);
                next_off = next_slot;
            }
        }

        // didn't find any slot that fit the specification. Find out if we can promote and by how much.
        let cur_size = self.granularity.size();

        let needed_size = cur_size + size as usize;
        if needed_size > LARGE_SIZE {
            Err(SlotError::TooBig)
        } else if needed_size > MEDIUM_SIZE {
            Err(SlotError::NeedsPromote(Granularity::Large))
        } else {
            Err(SlotError::NeedsPromote(Granularity::Medium))
        }
    }

    // utility function for merging adjacent slots.
    // slot are said to be adjacent if their offset plus their size
    // adds up to the offset of the next block.
    pub unsafe fn merge_adjacent_slots(&mut self) {
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
                    ptr::write(prev_slot,
                               EmptySlot {
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

    // Returns the unique granularity-index pair which describes this block.
    pub fn offset(&self) -> Offset {
        Offset::new(self.granularity, self.index)
    }

    // Initialize the block.
    pub unsafe fn initialize(&mut self, entity: Entity) {
        (**self.header).entity = entity;
    }
}

// The data blob is where all the component data is stored.
// It is a contiguous block of memory logically separated into "slabs" of the granularities above.
// Each granularity is given the same number of possible entries.
pub struct Blob {
    blocks_per_slab: usize,
    alloc: WorldAllocator,

    small: Slab,
    medium: Slab,
    large: Slab,
}

impl Blob {
    // Create a new blob. This does not allocate.
    pub fn new(alloc: WorldAllocator) -> Self {
        Blob {
            blocks_per_slab: 0,
            alloc: alloc,

            small: Slab::new(alloc, SMALL_SIZE),
            medium: Slab::new(alloc, MEDIUM_SIZE),
            large: Slab::new(alloc, LARGE_SIZE),
        }
    }

    // Returns the first free block in the given granularity's slab.
    // grows the buffer if necessary.
    pub fn next_block(&mut self, granularity: Granularity) -> BlockHandle {
        let slab = match granularity {
            Granularity::Small => &mut self.small,
            Granularity::Medium => &mut self.medium,
            Granularity::Large => &mut self.large,
        };
        
        let idx = slab.next_block();
        let mut block = unsafe { BlockHandle::from_raw(slab.get_block(idx), granularity, idx) };
        unsafe { block.clear() } // clear the block before we return it.
        
        block
    }

    // get a handle to live block. panics on index out of bounds.
    pub fn get_block(&self, offset: Offset) -> BlockHandle {
        let (gran, idx) = offset.into_parts();
        unsafe {
            let slab = match gran {
                Granularity::Small => &self.small,
                Granularity::Medium => &self.medium,
                Granularity::Large => &self.large,
            };
            
            debug_assert!(slab.is_alive(idx), "Attempted to get handle to dead block.");
            let block_ptr = slab.get_block(idx);
            
            BlockHandle::from_raw(block_ptr, gran, idx) 
        }
    }

    // Free a block. It must not be used again until it is next allocated.
    pub unsafe fn free_block(&mut self, block: BlockHandle) {
        let (gran, idx) = (block.granularity, block.index);
        
        match gran {
            Granularity::Small => self.small.mark_free(idx),
            Granularity::Medium => self.medium.mark_free(idx),
            Granularity::Large => self.large.mark_free(idx),
        }
    }

    // Promote a block to a higher granularity, growing if necessary.
    // Returns a handle to the new block.
    pub unsafe fn promote_block(&mut self, block: BlockHandle, new_gran: Granularity) -> BlockHandle {
        let old_size = block.granularity.size();
        let new_size = new_gran.size();
        assert!(old_size < new_size, "Promote called with invalid arguments.");
        assert!(block.granularity != Granularity::Large, "Attempted to promote max-size block.");
        
        let mut new_block = self.next_block(new_gran);
        
        // copy the data over, including the header to preserve freelist status, entity, etc.
        let old_header_ptr = *block.header as *mut u8;
        let new_header_ptr = *new_block.header as *mut u8;
        ptr::copy_nonoverlapping(old_header_ptr, new_header_ptr, block_size_with_header(old_size));
        
        // add a free slot to the end to make the new memory available.
        new_block.mark_free(old_size, new_size - old_size);
        
        // finally, free the old block and return the new.
        self.free_block(block);
        new_block
    }
}

unsafe impl Send for Blob {}
unsafe impl Sync for Blob {}

// Low space usage is very desirable for these offset tables.
// We limit the size of data blocks to be small enough that
// any possible offset is less than u16::MAX.
// we further limit the maximum offset to 14 bits,
// but this is artificial. 
const MAX_OFFSET: u16 = (1 << 15) - 1;
const CLEARED: u16 = ::std::u16::MAX;

/// Maps entities to the offset of this component's data
/// from the start of their block.
pub struct ComponentOffsetTable<T: Component> {
    offsets: Vector<u16, WorldAllocator>,
    _marker: PhantomData<T>,
}

impl<T: Component> ComponentOffsetTable<T> {
    pub fn new(alloc: WorldAllocator) -> Self {
        ComponentOffsetTable {
            offsets: Vector::with_alloc(alloc),
            _marker: PhantomData,
        }
    }

    /// Find the offset of the component this manages offsets for relative to the
    /// data of the entity this manages.
    pub fn offset_of(&self, entity: Entity) -> Option<u16> {
        // if the vector isn't long enough yet, that's a sure sign that
        // we haven't got an index for this entity.
        self.offsets.get(index_of(entity) as usize).and_then(|offset| {
            if *offset == CLEARED {
                None
            } else {
                Some(*offset)
            }
        })
    }

    /// Store the given offset for the entity given.
    pub fn set(&mut self, entity: Entity, offset: u16) {
        let idx = index_of(entity) as usize;
        // there shouldn't be any offsets that use this many bits.
        debug_assert!(idx <= MAX_OFFSET as usize);

        self.ensure_capacity(idx);
        self.offsets[idx] = offset;
    }

    /// Remove the offset for the entity given.
    pub fn remove(&mut self, entity: Entity) -> Option<u16> {
        use std::mem;

        let idx = index_of(entity) as usize;

        // don't bother extending the capacity if we don't even have
        // entries that far out.
        self.offsets
            .get_mut(idx)
            .map(|off| mem::replace(off, CLEARED))
            .and_then(|off| {
                if off == CLEARED {
                    None
                } else {
                    Some(off)
                }
            })
    }

    fn ensure_capacity(&mut self, size: usize) {
        while self.offsets.len() <= size {
            self.offsets.push(CLEARED);
        }
    }
}

#[cfg(test)]
mod tests {
}
