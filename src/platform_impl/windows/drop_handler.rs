use std::ffi::{c_void, OsString};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::core::{IUnknown, GUID, HRESULT};
use windows_sys::Win32::Foundation::{
    DV_E_FORMATETC, E_NOINTERFACE, E_POINTER, HWND, POINTL, S_OK,
};
use windows_sys::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows_sys::Win32::System::Ole::{CF_HDROP, DROPEFFECT_COPY, DROPEFFECT_NONE};
use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};

use tracing::debug;

use crate::platform_impl::platform::definitions::{
    IDataObjectVtbl, IDropTarget, IDropTargetVtbl, IUnknownVtbl,
};
use crate::platform_impl::platform::WindowId;

use crate::event::Event;
use crate::window::WindowId as RootWindowId;

// LISTARY PATCH (drop-target-query-interface): the interfaces `QueryInterface` answers for.
const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_IDROPTARGET: GUID = GUID::from_u128(0x00000122_0000_0000_c000_000000000046);

fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

#[repr(C)]
pub struct FileDropHandlerData {
    pub interface: IDropTarget,
    refcount: AtomicUsize,
    window: HWND,
    send_event: Box<dyn Fn(Event<()>)>,
    cursor_effect: u32,
    hovered_is_valid: bool, /* If the currently hovered item is not valid there must not be any
                             * `HoveredFileCancelled` emitted */
}

pub struct FileDropHandler {
    pub data: *mut FileDropHandlerData,
}

#[allow(non_snake_case)]
impl FileDropHandler {
    pub fn new(window: HWND, send_event: Box<dyn Fn(Event<()>)>) -> FileDropHandler {
        let data = Box::new(FileDropHandlerData {
            interface: IDropTarget { lpVtbl: &DROP_TARGET_VTBL as *const IDropTargetVtbl },
            refcount: AtomicUsize::new(1),
            window,
            send_event,
            cursor_effect: DROPEFFECT_NONE,
            hovered_is_valid: false,
        });
        FileDropHandler { data: Box::into_raw(data) }
    }

    // Implement IUnknown
    pub unsafe extern "system" fn QueryInterface(
        this: *mut IUnknown,
        riid: *const GUID,
        ppvObject: *mut *mut c_void,
    ) -> HRESULT {
        // LISTARY PATCH (drop-target-query-interface): was `unimplemented!()`. Third-party
        // modules that hook OLE call this on the pointer given to `RegisterDragDrop`, and a
        // panic cannot unwind out of `extern "system"`, so the process aborted.
        if ppvObject.is_null() {
            return E_POINTER;
        }
        unsafe { *ppvObject = ptr::null_mut() };
        if riid.is_null() {
            return E_POINTER;
        }
        let riid = unsafe { &*riid };
        if same_guid(riid, &IID_IUNKNOWN) || same_guid(riid, &IID_IDROPTARGET) {
            unsafe { Self::AddRef(this) };
            unsafe { *ppvObject = this as *mut c_void };
            S_OK
        } else {
            E_NOINTERFACE
        }
    }

    pub unsafe extern "system" fn AddRef(this: *mut IUnknown) -> u32 {
        let drop_handler_data = unsafe { Self::from_interface(this) };
        let count = drop_handler_data.refcount.fetch_add(1, Ordering::Release) + 1;
        count as u32
    }

    pub unsafe extern "system" fn Release(this: *mut IUnknown) -> u32 {
        let drop_handler = unsafe { Self::from_interface(this) };
        let count = drop_handler.refcount.fetch_sub(1, Ordering::Release) - 1;
        if count == 0 {
            // Destroy the underlying data
            drop(unsafe { Box::from_raw(drop_handler as *mut FileDropHandlerData) });
        }
        count as u32
    }

    pub unsafe extern "system" fn DragEnter(
        this: *mut IDropTarget,
        pDataObj: *const IDataObject,
        _grfKeyState: u32,
        _pt: *const POINTL,
        pdwEffect: *mut u32,
    ) -> HRESULT {
        use crate::event::WindowEvent::HoveredFile;
        let drop_handler = unsafe { Self::from_interface(this) };
        let hdrop = unsafe {
            Self::iterate_filenames(pDataObj, |filename| {
                drop_handler.send_event(Event::WindowEvent {
                    window_id: RootWindowId(WindowId(drop_handler.window)),
                    event: HoveredFile(filename),
                });
            })
        };
        drop_handler.hovered_is_valid = hdrop.is_some();
        drop_handler.cursor_effect =
            if drop_handler.hovered_is_valid { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        unsafe {
            *pdwEffect = drop_handler.cursor_effect;
        }

        S_OK
    }

    pub unsafe extern "system" fn DragOver(
        this: *mut IDropTarget,
        _grfKeyState: u32,
        _pt: *const POINTL,
        pdwEffect: *mut u32,
    ) -> HRESULT {
        let drop_handler = unsafe { Self::from_interface(this) };
        unsafe {
            *pdwEffect = drop_handler.cursor_effect;
        }

        S_OK
    }

    pub unsafe extern "system" fn DragLeave(this: *mut IDropTarget) -> HRESULT {
        use crate::event::WindowEvent::HoveredFileCancelled;
        let drop_handler = unsafe { Self::from_interface(this) };
        if drop_handler.hovered_is_valid {
            drop_handler.send_event(Event::WindowEvent {
                window_id: RootWindowId(WindowId(drop_handler.window)),
                event: HoveredFileCancelled,
            });
        }

        S_OK
    }

    pub unsafe extern "system" fn Drop(
        this: *mut IDropTarget,
        pDataObj: *const IDataObject,
        _grfKeyState: u32,
        _pt: *const POINTL,
        _pdwEffect: *mut u32,
    ) -> HRESULT {
        use crate::event::WindowEvent::DroppedFile;
        let drop_handler = unsafe { Self::from_interface(this) };
        let hdrop = unsafe {
            Self::iterate_filenames(pDataObj, |filename| {
                drop_handler.send_event(Event::WindowEvent {
                    window_id: RootWindowId(WindowId(drop_handler.window)),
                    event: DroppedFile(filename),
                });
            })
        };
        if let Some(hdrop) = hdrop {
            unsafe { DragFinish(hdrop) };
        }

        S_OK
    }

    unsafe fn from_interface<'a, InterfaceT>(this: *mut InterfaceT) -> &'a mut FileDropHandlerData {
        unsafe { &mut *(this as *mut _) }
    }

    unsafe fn iterate_filenames<F>(data_obj: *const IDataObject, callback: F) -> Option<HDROP>
    where
        F: Fn(PathBuf),
    {
        let drop_format = FORMATETC {
            cfFormat: CF_HDROP,
            ptd: ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT,
            lindex: -1,
            tymed: TYMED_HGLOBAL as u32,
        };

        let mut medium = unsafe { std::mem::zeroed() };
        let get_data_fn = unsafe { (*(*data_obj).cast::<IDataObjectVtbl>()).GetData };
        let get_data_result = unsafe { get_data_fn(data_obj as *mut _, &drop_format, &mut medium) };
        if get_data_result >= 0 {
            let hdrop = unsafe { medium.u.hGlobal as HDROP };

            // The second parameter (0xFFFFFFFF) instructs the function to return the item count
            let item_count = unsafe { DragQueryFileW(hdrop, 0xffffffff, ptr::null_mut(), 0) };

            for i in 0..item_count {
                // Get the length of the path string NOT including the terminating null character.
                // Previously, this was using a fixed size array of MAX_PATH length, but the
                // Windows API allows longer paths under certain circumstances.
                let character_count =
                    unsafe { DragQueryFileW(hdrop, i, ptr::null_mut(), 0) as usize };
                let str_len = character_count + 1;

                // Fill path_buf with the null-terminated file name
                let mut path_buf = Vec::with_capacity(str_len);
                unsafe {
                    DragQueryFileW(hdrop, i, path_buf.as_mut_ptr(), str_len as u32);
                    path_buf.set_len(str_len);
                }

                callback(OsString::from_wide(&path_buf[0..character_count]).into());
            }

            Some(hdrop)
        } else if get_data_result == DV_E_FORMATETC {
            // If the dropped item is not a file this error will occur.
            // In this case it is OK to return without taking further action.
            debug!("Error occurred while processing dropped/hovered item: item is not a file.");
            None
        } else {
            debug!("Unexpected error occurred while processing dropped/hovered item.");
            None
        }
    }
}

impl FileDropHandlerData {
    fn send_event(&self, event: Event<()>) {
        (self.send_event)(event);
    }
}

impl Drop for FileDropHandler {
    fn drop(&mut self) {
        unsafe {
            FileDropHandler::Release(self.data as *mut IUnknown);
        }
    }
}

static DROP_TARGET_VTBL: IDropTargetVtbl = IDropTargetVtbl {
    parent: IUnknownVtbl {
        QueryInterface: FileDropHandler::QueryInterface,
        AddRef: FileDropHandler::AddRef,
        Release: FileDropHandler::Release,
    },
    DragEnter: FileDropHandler::DragEnter,
    DragOver: FileDropHandler::DragOver,
    DragLeave: FileDropHandler::DragLeave,
    Drop: FileDropHandler::Drop,
};

// LISTARY PATCH (drop-target-query-interface): calls `QueryInterface` through the vtable, as an
// OLE hook does. Without the patch the first call aborts the test process.
#[cfg(test)]
mod tests {
    use super::*;

    fn handler() -> FileDropHandler {
        FileDropHandler::new(0, Box::new(|_| {}))
    }

    fn unknown(handler: &FileDropHandler) -> *mut IUnknown {
        handler.data as *mut IUnknown
    }

    fn refcount(handler: &FileDropHandler) -> usize {
        unsafe { (*handler.data).refcount.load(Ordering::Acquire) }
    }

    unsafe fn query(this: *mut IUnknown, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
        let vtbl = unsafe { &*(*(this as *mut IDropTarget)).lpVtbl };
        unsafe { (vtbl.parent.QueryInterface)(this, riid, ppv) }
    }

    unsafe fn release(this: *mut c_void) -> u32 {
        let vtbl = unsafe { &*(*(this as *mut IDropTarget)).lpVtbl };
        unsafe { (vtbl.parent.Release)(this as *mut IUnknown) }
    }

    #[test]
    fn iunknown_and_idroptarget_give_the_same_pointer() {
        let handler = handler();
        let this = unknown(&handler);
        let mut as_unknown = ptr::null_mut();
        let mut as_drop_target = ptr::null_mut();
        assert_eq!(unsafe { query(this, &IID_IUNKNOWN, &mut as_unknown) }, S_OK);
        assert_eq!(unsafe { query(this, &IID_IDROPTARGET, &mut as_drop_target) }, S_OK);
        assert_eq!(as_unknown, this as *mut c_void);
        assert_eq!(as_drop_target, this as *mut c_void);
        assert_eq!(refcount(&handler), 3);
        assert_eq!(unsafe { release(as_unknown) }, 2);
        assert_eq!(unsafe { release(as_drop_target) }, 1);
    }

    #[test]
    fn other_interfaces_are_refused_and_the_output_cleared() {
        // IDataObject: a real interface the drop target does not implement.
        let iid_idataobject = GUID::from_u128(0x0000010e_0000_0000_c000_000000000046);
        let handler = handler();
        let mut out = 1usize as *mut c_void;
        assert_eq!(unsafe { query(unknown(&handler), &iid_idataobject, &mut out) }, E_NOINTERFACE);
        assert!(out.is_null());
        assert_eq!(refcount(&handler), 1);
    }

    #[test]
    fn null_pointers_get_e_pointer() {
        let handler = handler();
        let this = unknown(&handler);
        assert_eq!(unsafe { query(this, &IID_IUNKNOWN, ptr::null_mut()) }, E_POINTER);
        let mut out = 1usize as *mut c_void;
        assert_eq!(unsafe { query(this, ptr::null(), &mut out) }, E_POINTER);
        assert!(out.is_null());
        assert_eq!(refcount(&handler), 1);
    }
}
