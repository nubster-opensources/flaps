#![no_main]

use flaps_eval::FlagSet;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let Ok(document) = std::str::from_utf8(input) else {
        return;
    };

    let Ok(parsed) = FlagSet::from_json(document) else {
        return;
    };

    let canonical = parsed.to_json();
    let reparsed = FlagSet::from_json(&canonical)
        .expect("a successfully parsed flag set must serialize to a valid flag set");
    assert_eq!(parsed, reparsed);
});
