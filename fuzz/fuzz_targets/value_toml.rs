#![no_main]

use libfuzzer_sys::fuzz_target;
use modconflict::value::{load, Format};

// TOML caps its own recursion, so this is the control: any panic here is a bug
// in our conversion, not the parser.
fuzz_target!(|data: &[u8]| {
    let _ = load(data, Format::Toml);
});
