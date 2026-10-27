#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| uptime_kinomi::fuzzing::http_request(data));
