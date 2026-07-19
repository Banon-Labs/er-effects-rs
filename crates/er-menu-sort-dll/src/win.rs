use std::ffi::c_void;

pub(crate) type HInstance = *mut c_void;

const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;
const FALSE: i32 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GameAddress(usize);

impl GameAddress {
    pub(crate) const fn new(value: usize) -> Self {
        Self(value)
    }

    pub(crate) const fn value(self) -> usize {
        self.0
    }

    pub(crate) const fn is_null(self) -> bool {
        self.0 == 0
    }

    pub(crate) const fn offset(self, offset: usize) -> Self {
        Self(self.0 + offset)
    }

    fn as_const_ptr(self) -> *const c_void {
        self.0 as *const c_void
    }

    fn as_mut_ptr(self) -> *mut c_void {
        self.0 as *mut c_void
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GameModule {
    base: GameAddress,
}

impl GameModule {
    pub(crate) fn current() -> Result<Self, String> {
        let module = unsafe { GetModuleHandleA(std::ptr::null()) };
        if module.is_null() {
            Err("failed to resolve game module".to_owned())
        } else {
            Ok(Self {
                base: GameAddress::new(module as usize),
            })
        }
    }

    pub(crate) const fn rva(self, rva: usize) -> GameAddress {
        self.base.offset(rva)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProcessMemory;

impl ProcessMemory {
    pub(crate) fn read_address(self, address: GameAddress) -> Option<GameAddress> {
        self.read_usize(address)
            .map(GameAddress::new)
            .filter(|address| !address.is_null())
    }

    pub(crate) fn read_i32(self, address: GameAddress) -> Option<i32> {
        let mut value = 0_i32;
        self.read_into(address, &mut value).then_some(value)
    }

    pub(crate) fn write_u32(self, address: GameAddress, value: u32) -> bool {
        let mut written = 0_usize;
        let ok = unsafe {
            WriteProcessMemory(
                CURRENT_PROCESS_PSEUDO_HANDLE,
                address.as_mut_ptr(),
                &value as *const u32 as *const c_void,
                std::mem::size_of::<u32>(),
                &mut written,
            )
        };
        ok != FALSE && written == std::mem::size_of::<u32>()
    }

    fn read_usize(self, address: GameAddress) -> Option<usize> {
        let mut value = 0_usize;
        self.read_into(address, &mut value).then_some(value)
    }

    fn read_into<T>(self, address: GameAddress, out: &mut T) -> bool {
        let mut read = 0_usize;
        let ok = unsafe {
            ReadProcessMemory(
                CURRENT_PROCESS_PSEUDO_HANDLE,
                address.as_const_ptr(),
                out as *mut T as *mut c_void,
                std::mem::size_of::<T>(),
                &mut read,
            )
        };
        ok != FALSE && read == std::mem::size_of::<T>()
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleA(module_name: *const u8) -> *mut c_void;

    fn ReadProcessMemory(
        process: isize,
        base: *const c_void,
        buffer: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;

    fn WriteProcessMemory(
        process: isize,
        base: *mut c_void,
        buffer: *const c_void,
        size: usize,
        written: *mut usize,
    ) -> i32;
}
