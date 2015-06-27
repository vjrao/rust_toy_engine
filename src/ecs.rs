use std::collections::HashSet;

/// An unique entity.
/// Entities each have a unique id which serves
/// as a weak pointer to the entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Entity {
    id: usize,
}

/// Simple entity manager backed by a hashmap.
pub struct EntityManager {
	next: Entity,
	entities: HashSet<Entity>,
}

impl EntityManager {
	/// Creates a new entity manager.
	pub fn new() -> EntityManager {
		EntityManager { 
			next: Entity { id: 0 },
			entities: HashSet::new()
		}
	}
    /// Creates an entity.
    pub fn next(&mut self) -> Entity {
    	while self.alive(&self.next) {
    		self.next.id += 1;
    	}
    	self.entities.insert(self.next);
    	self.next
    }

    /// Creates a vector of n entities.
    pub fn next_n(&mut self, n: usize) -> Vec<Entity> {
    	let mut entities = Vec::with_capacity(n);
    	for _ in 0..n {
    		entities.push(self.next());
    	}
    	entities
    }

    /// Whether an entity is currently "alive", or exists.
    pub fn alive(&self, e: &Entity) -> bool {
    	self.entities.contains(e)
    }
}

#[cfg(test)]
mod tests {

}