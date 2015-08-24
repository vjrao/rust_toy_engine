Build a database of entities and component data as byte arrays
==============================================================
implementation 1: 
-----------------
offsets: (pos_offset, pos_len), (vel_offset, vel_len), ...
stores a BST or similar structure (b-tree?) for each entity's offsets.
there should be a large byte array holding each entity's component data.

data format: [e1.pos, e1.vel][e2.pos][e3.vel]...

lookup for a specific entity: 
search_tree.find_offsets(my_entity)
  
query:
```
for e in all_entities.map(|e| lookup(e)) { if has all { add to set } }
```
NOTES: 
  - query is likely inefficient because you have to loop through all entities
  - data is laid out in a fairly cache-friendly way.
  - uses a lot of memory as it has to store a None for every component
    that an entity doesn't even have.

implementation 2:
-----------------
each column has its own search tree of entities holding an (offset, len) pair.
each column also has its own data array.
    
data layout: [e1.pos, e2.pos, e3.pos, ...], [e1.vel, e2.vel, e3.vel, ...]

query:
  find overlap between all requested columns.

NOTES:
  - data is still moderately cache-friendly
  - only stores as much memory as it needs to
  - finding overlap is tricky, but may not be overly expensive.

implementation 2 seems like it would be faster and more memory efficient.

additionally, we can store records of what queries each system has run.
we can populate this table with results for queries that are typically run as well. "typically" being (times_run_out_of_last_100 / 100) > EPSILON where EPSILON is some threshold.

a query run now goes like this:
  - create a specification e.g. (with_component(T), filtered(T))
  - find the parts of the specification that are contained in the database.
  - run the parts of the specification that are not contained in the database.
  - find the overlap between all of these.

  this will cache common queries. should we cache the entire query, or just parts? 

  if we cache entire queries, we eliminate needing to find overlaps multiple times, but this only helps if the same query is run multiple times, which doesn't seem excessively common. if we just cache parts of the query, we still need to find overlap between all parts, but multiple queries which use similar parts will all benefit. we could even start by pre-processing zero columns of the database before each update, and then applying all that are used often enough. No point in caching all entities if they aren't used often.

Batching edits:
===============
  seems hard to do generically, but we can provide an interface for users to apply the same edit to many entities. one of the most expensive parts of sending edits is the act of actually sending it over the channel, which involves expensive memory allocations. there should be a way to apply *similar* edits to multiple entities, e.g. apply a translate of (entity.velocity.x, entity.velocity.y) to entity.position for every entity with both.This will reduce the number of messages sent over the channel to 1 from len(entities), which should yield a large performance improvement.