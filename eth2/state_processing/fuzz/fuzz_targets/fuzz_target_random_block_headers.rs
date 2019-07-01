#![no_main]
#[macro_use] extern crate libfuzzer_sys;
extern crate ssz;
extern crate state_processing;
extern crate state_processing_fuzz;
extern crate types;

use ssz::{Decode, Encode};
use state_processing_fuzz::from_minimal_state_file;
use types::*;
use types::test_utils::TestingBeaconBlockBuilder;
use state_processing::process_block_header;


// Fuzz `per_block_processing()`
fuzz_target!(|data: &[u8]| {
    // Convert data to a BeaconBlock
    let block = BeaconBlock::from_ssz_bytes(&data);

    if !block.is_err() {
        println!("Processing block header");
        // Generate a chain_spec
        let spec = MinimalEthSpec::default_spec();
        let mut state = from_minimal_state_file(&spec);

        // Fuzz per_block_processing (if decoding was successful)
        let block = &block.unwrap();
        println!("Valid block header? {}", !process_block_header(&mut state, &block, &spec, true).is_err());
    }
});

// Code for generating a BeaconBlock (use as a corpus)
pub fn generate_block_header() {
    let spec = MinimalEthSpec::default_spec();

    let mut state = from_minimal_state_file(&spec);

    let mut builder = TestingBeaconBlockBuilder::new(&spec);

    builder.set_slot(state.slot);

    let block = builder.build();

    assert!(!process_block_header(&mut state, &block, &spec, true).is_err());
    println!("Block {}", hex::encode(block.as_ssz_bytes()));
}
