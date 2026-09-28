#![no_std]
#![no_main]

extern crate libruntime;

#[used]
static RUNTIME_ENTRY: unsafe extern "C" fn(*const u8, usize) -> ! = libruntime::__runtime_start;

use libkernel::preclude::*;

fn app_main() {
    debug!("sifive_spi!");
}

libruntime::entry!(app_main);
