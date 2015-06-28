use super::{Size, Window, GLContext, WindowSettings, InputEvent };
use ::glutin;

/// An implementation for `Window` backed by Glutin
pub struct GlutinWindow {
	internal: glutin::Window,
	title: String,
	should_close: bool,
	capture_cursor: bool,
}

impl GlutinWindow {
	/// Get a handle to the internal glutin window backing this struct.
	pub fn get_internal<'a>(&'a self) -> &'a glutin::Window {
		&self.internal
	}
}

impl From<WindowSettings> for GlutinWindow {
	fn from(settings: WindowSettings) -> GlutinWindow {
		GlutinWindow::new(settings)
	}
}

impl Window for GlutinWindow {
	fn new(settings: WindowSettings) -> GlutinWindow {
		GlutinWindow {
			internal: glutin::Window::new().unwrap(),
			title: "".into(),
			should_close: false,
			capture_cursor: false,
		}
	}

	fn should_close(&self) -> bool {
		self.should_close
	}

	fn size(&self) -> Size {
		match self.internal.get_outer_size() {
			Some((w,h)) => (w,h).into(),
			None => (0, 0).into()
		}
	}

	fn inner_size(&self) -> Size {
		let hi_dpi = self.internal.hidpi_factor();
		if let Some((w, h)) = self.internal.get_inner_size() {
			((w as f32 * hi_dpi) as u32, (h as f32 * hi_dpi) as u32).into()
		} else {
			(0, 0).into()
		}
	}

	fn title(&self) -> String {
		self.title.clone()
	}

	fn set_title(&mut self, title: String) {
		self.internal.set_title(&title);
		self.title = title;
	}

	fn set_capture_cursor(&mut self, capture: bool) {
		self.capture_cursor = capture;

		//todo: use result here
		self.internal.set_cursor_state(match capture {
			true => glutin::CursorState::Grab,
			false => glutin::CursorState::Normal,
		});
	}

	fn poll_event(&mut self) -> Option<InputEvent> {
		use glutin::Event;
		//implement this for real once event system is fleshed out
		match self.internal.poll_events().next() {
			Some(Event::Closed) => { self.should_close = true; None }
			_ => None
		}
	}
}

impl GLContext for GlutinWindow {
	fn get_proc_address(&self, addr: &str) -> *const ::libc::c_void {
		self.internal.get_proc_address(addr)
	}

	unsafe fn make_current(&self) {
		self.internal.make_current();
	}

	fn is_current(&self) -> bool {
		self.internal.is_current()
	}

	fn swap_buffers(&self) {
		self.internal.swap_buffers();
	}

}