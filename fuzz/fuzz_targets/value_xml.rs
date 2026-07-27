#![no_main]

use libfuzzer_sys::fuzz_target;
use modconflict::value::{load, Format};

// roxmltree overflows the stack on a deeply nested document before it can
// error; the depth check runs first. Fuzzing proves nothing gets past it.
fuzz_target!(|data: &[u8]| {
    let _ = load(data, Format::Xml);
});
