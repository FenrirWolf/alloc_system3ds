// Copyright 2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![crate_name = "alloc_system"]
#![crate_type = "rlib"]
#![no_std]
#![deny(warnings)]
#![feature(allocator_api)]
#![feature(alloc)]
#![feature(core_intrinsics)]
#![feature(libc)]

// The minimum alignment guaranteed by the architecture. This value is used to
// add fast paths for low alignment values. In practice, the alignment is a
// constant at the call site and the branch will be optimized out.
#[cfg(all(any(target_arch = "x86",
              target_arch = "arm",
              target_arch = "mips",
              target_arch = "powerpc",
              target_arch = "powerpc64",
              target_arch = "asmjs",
              target_arch = "wasm32")))]
const MIN_ALIGN: usize = 8;
#[cfg(all(any(target_arch = "x86_64",
              target_arch = "aarch64",
              target_arch = "mips64",
              target_arch = "s390x",
              target_arch = "sparc64")))]
const MIN_ALIGN: usize = 16;


extern crate alloc;

use alloc::heap::{Alloc, AllocErr, Layout, Excess, CannotReallocInPlace};

pub struct System;

unsafe impl Alloc for System {
    #[inline]
    unsafe fn alloc(&mut self, layout: Layout) -> Result<*mut u8, AllocErr> {
        (&*self).alloc(layout)
    }

    #[inline]
    unsafe fn alloc_zeroed(&mut self, layout: Layout)
        -> Result<*mut u8, AllocErr>
    {
        (&*self).alloc_zeroed(layout)
    }

    #[inline]
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        (&*self).dealloc(ptr, layout)
    }

    #[inline]
    unsafe fn realloc(&mut self,
                      ptr: *mut u8,
                      old_layout: Layout,
                      new_layout: Layout) -> Result<*mut u8, AllocErr> {
        (&*self).realloc(ptr, old_layout, new_layout)
    }

    fn oom(&mut self, err: AllocErr) -> ! {
        (&*self).oom(err)
    }

    #[inline]
    fn usable_size(&self, layout: &Layout) -> (usize, usize) {
        (&self).usable_size(layout)
    }

    #[inline]
    unsafe fn alloc_excess(&mut self, layout: Layout) -> Result<Excess, AllocErr> {
        (&*self).alloc_excess(layout)
    }

    #[inline]
    unsafe fn realloc_excess(&mut self,
                             ptr: *mut u8,
                             layout: Layout,
                             new_layout: Layout) -> Result<Excess, AllocErr> {
        (&*self).realloc_excess(ptr, layout, new_layout)
    }

    #[inline]
    unsafe fn grow_in_place(&mut self,
                            ptr: *mut u8,
                            layout: Layout,
                            new_layout: Layout) -> Result<(), CannotReallocInPlace> {
        (&*self).grow_in_place(ptr, layout, new_layout)
    }

    #[inline]
    unsafe fn shrink_in_place(&mut self,
                              ptr: *mut u8,
                              layout: Layout,
                              new_layout: Layout) -> Result<(), CannotReallocInPlace> {
        (&*self).shrink_in_place(ptr, layout, new_layout)
    }
}

mod platform {
    extern crate libc;

    use core::cmp;
    use core::ptr;

    use MIN_ALIGN;
    use ::System;
    use ::alloc::heap::{Alloc, AllocErr, Layout};

    unsafe impl<'a> Alloc for &'a System {
        #[inline]
        unsafe fn alloc(&mut self, layout: Layout) -> Result<*mut u8, AllocErr> {
            let ptr = if layout.align() <= MIN_ALIGN {
                libc::malloc(layout.size()) as *mut u8
            } else {
                aligned_malloc(&layout)
            };
            if !ptr.is_null() {
                Ok(ptr)
            } else {
                Err(AllocErr::Exhausted { request: layout })
            }
        }

        #[inline]
        unsafe fn alloc_zeroed(&mut self, layout: Layout)
            -> Result<*mut u8, AllocErr>
        {
            if layout.align() <= MIN_ALIGN {
                let ptr = libc::calloc(layout.size(), 1) as *mut u8;
                if !ptr.is_null() {
                    Ok(ptr)
                } else {
                    Err(AllocErr::Exhausted { request: layout })
                }
            } else {
                let ret = self.alloc(layout.clone());
                if let Ok(ptr) = ret {
                    ptr::write_bytes(ptr, 0, layout.size());
                }
                ret
            }
        }

        #[inline]
        unsafe fn dealloc(&mut self, ptr: *mut u8, _layout: Layout) {
            libc::free(ptr as *mut libc::c_void)
        }

        #[inline]
        unsafe fn realloc(&mut self,
                          ptr: *mut u8,
                          old_layout: Layout,
                          new_layout: Layout) -> Result<*mut u8, AllocErr> {
            if old_layout.align() != new_layout.align() {
                return Err(AllocErr::Unsupported {
                    details: "cannot change alignment on `realloc`",
                })
            }

            if new_layout.align() <= MIN_ALIGN {
                let ptr = libc::realloc(ptr as *mut libc::c_void, new_layout.size());
                if !ptr.is_null() {
                    Ok(ptr as *mut u8)
                } else {
                    Err(AllocErr::Exhausted { request: new_layout })
                }
            } else {
                let res = self.alloc(new_layout.clone());
                if let Ok(new_ptr) = res {
                    let size = cmp::min(old_layout.size(), new_layout.size());
                    ptr::copy_nonoverlapping(ptr, new_ptr, size);
                    self.dealloc(ptr, old_layout);
                }
                res
            }
        }

        fn oom(&mut self, err: AllocErr) -> ! {
            use core::fmt::{self, Write};

            // Print a message to stderr before aborting to assist with
            // debugging. It is critical that this code does not allocate any
            // memory since we are in an OOM situation. Any errors are ignored
            // while printing since there's nothing we can do about them and we
            // are about to exit anyways.
            drop(writeln!(Stderr, "fatal runtime error: {}", err));
            unsafe {
                ::core::intrinsics::abort();
            }

            struct Stderr;

            impl Write for Stderr {
                fn write_str(&mut self, s: &str) -> fmt::Result {
                    unsafe {
                        libc::write(libc::STDERR_FILENO,
                                    s.as_ptr() as *const libc::c_void,
                                    s.len());
                    }
                    Ok(())
                }
            }
        }
    }

    #[cfg(any(target_os = "android", target_os = "redox", target_env = "newlib"))]
    #[inline]
    unsafe fn aligned_malloc(layout: &Layout) -> *mut u8 {
        // On android we currently target API level 9 which unfortunately
        // doesn't have the `posix_memalign` API used below. Instead we use
        // `memalign`, but this unfortunately has the property on some systems
        // where the memory returned cannot be deallocated by `free`!
        //
        // Upon closer inspection, however, this appears to work just fine with
        // Android, so for this platform we should be fine to call `memalign`
        // (which is present in API level 9). Some helpful references could
        // possibly be chromium using memalign [1], attempts at documenting that
        // memalign + free is ok [2] [3], or the current source of chromium
        // which still uses memalign on android [4].
        //
        // [1]: https://codereview.chromium.org/10796020/
        // [2]: https://code.google.com/p/android/issues/detail?id=35391
        // [3]: https://bugs.chromium.org/p/chromium/issues/detail?id=138579
        // [4]: https://chromium.googlesource.com/chromium/src/base/+/master/
		//                                       /memory/aligned_memory.cc
        libc::memalign(layout.align(), layout.size()) as *mut u8
    }

    #[cfg(not(any(target_os = "android", target_os = "redox", target_env = "newlib")))]
    #[inline]
    unsafe fn aligned_malloc(layout: &Layout) -> *mut u8 {
        let mut out = ptr::null_mut();
        let ret = libc::posix_memalign(&mut out, layout.align(), layout.size());
        if ret != 0 {
            ptr::null_mut()
        } else {
            out as *mut u8
        }
    }
}
