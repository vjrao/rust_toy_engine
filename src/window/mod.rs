/// A size.
#[derive(Copy, Clone)]
pub struct Size {
	/// The width.
	width: u32,
	/// The height.
	height: u32,
}

/// A generic window trait.
pub trait Window {
	//TODO: Event handling? Window creation?
	
	/// Whether the window should close.
	fn should_close(&self) -> bool;
	
	/// The size of the window.
	fn size(&self) -> Size

	/// Swaps render buffers.
	fn swap_buffers(&mut self);

	/// The size of the drawing area of the window.
	fn draw_size(&self);

	/// Gets the current title of the window.
	fn get_title(&self) -> String;

	/// Sets the title of the window.
	fn set_title(&mut self, title: String);

	/// Whether to capture and hide the cursor.
	fn set_capture_cursor(&mut self, capture: bool);
}

pub trait GLContext {
	/// Returns the address of an OpenGL function or a null pointer.
	fn get_proc_address(&mut self, proc_name: &str) -> *const libc::c_void;

	/// Makes this context the current one on this thread.
	unsafe fn make_current(&mut self);

	/// Whether this context is the current 
	fn is_current(&self) -> bool;
}