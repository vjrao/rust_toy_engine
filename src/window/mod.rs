
#[cfg(feature = "glutin")]
mod glutin_window;

#[cfg(feature = "glutin")]
pub use self::glutin_window::GlutinWindow;

//placeholders
pub type InputEvent = ();

/// A size.
#[derive(Copy, Clone)]
pub struct Size {
	/// The width.
	pub width: u32,
	/// The height.
	pub height: u32,
}

impl From<(u32, u32)> for Size {
	fn from(t: (u32, u32)) -> Size {
		Size { width: t.0, height: t.1 }
	}
}

impl From<[u32; 2]> for Size {
	fn from(t: [u32; 2]) -> Size {
		Size { width: t[0], height: t[1] }
	}
}


/// Settings for creating a window
#[derive(Clone)]
pub struct WindowSettings {
	size: Size,
	title: String,
	fullscreen: bool,
	vsync: bool,
	gl: (u8, u8)
}

impl WindowSettings {
	/// Creates a new WindowSettings with default params
	pub fn new() -> WindowSettings {
		WindowSettings {
			size: (800, 600).into(),
			title: "".into(),
			fullscreen: false,
			vsync: false,
			gl: (3, 3)
		}
	}

	/// Specify a size
	pub fn size(mut self, s: Size) -> WindowSettings {
		self.size = s;
		self
	}

	/// Specify a title
	pub fn title(mut self, t: String) -> WindowSettings {
		self.title = t;
		self
	}
}

impl Default for WindowSettings {
	fn default() -> WindowSettings {
		WindowSettings::new()
	}
}

/// A generic window trait.
pub trait Window {
	//todo: error handling?

	fn new(settings: WindowSettings) -> Self;
	
	/// Whether the window should close.
	fn should_close(&self) -> bool;
	
	/// The size of the window.
	fn size(&self) -> Size;

	/// The size of the area of the window, minus the border and title bar.
	fn inner_size(&self) -> Size;

	/// Gets the current title of the window.
	fn title(&self) -> String;

	/// Sets the title of the window.
	fn set_title(&mut self, title: String);

	/// Whether to capture and hide the cursor.
	fn set_capture_cursor(&mut self, capture: bool);

	/// Polls for the next event
	fn poll_event(&mut self) -> Option<InputEvent>;
}

/// Things that manage or represent OpenGL contexts
pub trait GLContext {
	//todo: error handling?

	/// Returns the address of an OpenGL function or a null pointer.
	fn get_proc_address(&self, addr: &str) -> *const ::libc::c_void;

	/// Makes this context the current one on this thread.
	unsafe fn make_current(&self);

	/// Whether this context is the current 
	fn is_current(&self) -> bool;

	/// Swaps render buffers.
	fn swap_buffers(&self);
}