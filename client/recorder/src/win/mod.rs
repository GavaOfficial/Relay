pub mod aac;
pub mod audio;
pub mod capture;
pub mod convert;
pub mod device;
pub mod encoder;
pub mod engine;
pub mod probe;
pub mod window;

pub struct Unsync<T>(pub T);

unsafe impl<T> Send for Unsync<T> {}
unsafe impl<T> Sync for Unsync<T> {}

pub fn com_init() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
    }
}
