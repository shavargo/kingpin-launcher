//! Build script — embed the Windows `.ico` into the PE resource table so the
//! `.exe` shows the Kingpin icon in Explorer / the taskbar without a sidecar file.
//!
//! Failure is a warning, not a hard error, so the project still builds if
//! `winresource` tools are missing (e.g. odd CI images). The runtime still
//! loads `assets/icon.png` via egui for the window chrome.

fn main() {
	#[cfg(windows)]
	{
		let mut res = winresource::WindowsResource::new();
		res.set_icon("assets/icon.ico");
		if let Err(e) = res.compile() {
			eprintln!("cargo:warning=winresource icon embed failed: {e}");
		}
	}
}
