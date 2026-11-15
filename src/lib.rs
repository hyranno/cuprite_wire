pub mod parts;
pub mod subcircuit;
pub mod circuit;
pub mod sexpr;

use std::panic::Location;


#[track_caller]
pub fn get_location() -> &'static Location<'static> {
    Location::caller()
}

