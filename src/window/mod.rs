
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


/// Settings for creating a window.
#[derive(Clone)]
pub struct WindowSettings {
	pub size: Size,
	pub title: String,
	pub fullscreen: bool,
	pub vsync: bool,
	pub gl: (u8, u8)
}

impl WindowSettings {
	/// Creates a new WindowSettings with default params.
	pub fn new() -> WindowSettings {
		WindowSettings {
			size: (800, 600).into(),
			title: "".into(),
			fullscreen: false,
			vsync: false,
			gl: (3, 3)
		}
	}

	/// Specify a size.
	pub fn with_size(self, s: Size) -> WindowSettings {
		WindowSettings { size: s, ..self }
	}

	/// Specify a title.
	pub fn with_title(self, t: String) -> WindowSettings {
		WindowSettings { title: t, ..self }
	}

	/// Whether the window should be fullscreen.
	pub fn with_fullscreen(self, f: bool) -> WindowSettings {
		WindowSettings { fullscreen: f, ..self }
	}

	/// Whether the window should render in vsync.
	pub fn with_vsync(self, v: bool) -> WindowSettings {
		WindowSettings { vsync: v, ..self }
	}

	/// Specify an OpenGL version for this window to be created with.
	pub fn with_gl_version(self, v: (u8, u8)) -> WindowSettings {
		WindowSettings { gl: v, ..self }
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