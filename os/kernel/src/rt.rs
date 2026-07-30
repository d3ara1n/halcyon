use core::{alloc::Layout, panic::PanicInfo};

use buddy_system_allocator::{Heap, LockedHeapWithRescue};
use dtb_parser::DeviceTree;
use erhino_shared::proc::Termination;

use crate::{
    board::{self},
    external::{_heap_start, _stack_start},
    fs, hart,
    mm::{
        self,
        frame::{self, alloc},
        page::PAGE_SIZE,
    },
    println, sbi,
};

const HEAP_ORDER: usize = 32;

#[global_allocator]
static mut HEAP_ALLOCATOR: LockedHeapWithRescue<HEAP_ORDER> =
    LockedHeapWithRescue::new(heap_rescue);

#[lang = "start"]
fn rust_start<T: Termination + 'static>(
    main: fn() -> T,
    _: isize,
    argv: *const *const u8,
    _sigpipe: u8,
) -> isize {
    let dtb_addr = argv as usize;
    // rust initialization
    unsafe {
        let heap_start = _heap_start as usize;
        let size = _stack_start as usize - heap_start;
        HEAP_ALLOCATOR.lock().init(heap_start, size);
    }
    early_init(dtb_addr);
    kernel_init();
    main();
    hart::start_all();
    hart::enter_user();
}

fn early_init(dtb_addr: usize) {
    sbi::init();
    let tree = DeviceTree::from_address(dtb_addr).expect("device tree not available");
    board::init(tree);
    let board = board::this_board();
    if let Some((addr, _)) = board.initfs() {
        frame::init(addr);
    }else{
        panic!("no initfs info");
    }
    hart::init();
}

fn kernel_init() {
    mm::init();
    fs::init();
}

#[panic_handler]
fn handle_panic(info: &PanicInfo) -> ! {
    if let Some(location) = info.location() {
        println!(
            "\x1b[0;31mKernel panicking #{} \x1b[0m\nin file {} at line {}: {}",
            hart::hartid(),
            location.file(),
            location.line(),
            info.message()
        );
    } else {
        println!(
            "\x1b[0;31mKernel panicking #{}: \x1b[0mno information available.",
            hart::hartid()
        );
    }
    loop {}
}

#[alloc_error_handler]
pub fn handle_alloc_error(layout: Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}

fn heap_rescue(heap: &mut Heap<HEAP_ORDER>, layout: &Layout) {
    let single = 1 * PAGE_SIZE;
    let mut size = 1;
    let required = layout.size();
    unsafe {
        while required > size * single {
            size *= 2;
        }
        if let Some(frame_start) = alloc(size) {
            heap.add_to_heap(frame_start * single, (frame_start + size) * single);
        } else {
            panic!("kernel memory request but ran out of memory");
        }
    }
}
