#![no_main]

use libfuzzer_sys::fuzz_target;
use modconflict::value::{load, Format};

// Every metadata file was downloaded by someone who wanted a nicer sword, so
// hostile JSON is the default. It must always be an error, never a panic or a
// stack overflow — the JSONC parser recurses, and the depth guard in front of
// it is exactly what this exercises.
fuzz_target!(|data: &[u8]| {
    let _ = load(data, Format::Json);
});
