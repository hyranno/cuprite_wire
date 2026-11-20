use cuprite_wire::subcircuit::Interface;

#[derive(Interface)]
enum NotAStruct {
    A,
}

#[derive(Interface)]
struct Tuple<T>(T, T);

#[derive(Interface)]
struct NoParameter {
    input: u32,
}

#[derive(Interface)]
struct TwoParameters<T, U> {
    input: T,
    output: U,
}

#[derive(Interface)]
struct Bounded<T: Clone> {
    input: T,
}

#[derive(Interface)]
struct OtherField<T> {
    input: T,
    count: u32,
}

fn main() {}
