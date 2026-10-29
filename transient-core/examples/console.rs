use transient_core::*;

/// A program with a console: `hello` greets every time it is typed, `sum` asks for two numbers, `quit` ends it.
/// Try it by hand (`cargo run --example console`) or with `printf 'hello hello sum 2 3 quit\n' | ...`
fn main() {
    let program = option("hello", "greets")
        .map(|_| println!("hello!"))
        | tdo! {
            option("sum", "adds two numbers");
            let a = input::<i32>("first? ");
            let b = input::<i32>("second? ");
            lift_io(move || println!("{a} + {b} = {}", a + b))
        }
        | tdo! {
            option1("quit", "ends the program");
            exit("bye")
        };
    let r: Result<&str, String> = keep(program);
    println!("keep finished with {r:?}");
}
