#ECS

An entity component system, attempting to be completely type-safe, multithreaded, and performant.
The brunt of the work in this module is done in user-provided `Processors`. The `World` manages entities and component data.

The world can be used to create an execution context, which is then supplied with the processors to execute. It provides
control-flow utilities for defining a strict order for processors to execute in, while also allowing for non-competing processors
to execute their code efficiently across multiple threads.