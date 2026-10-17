# easyfix-macros

Proc macros for the [easyfix](https://github.com/ldanko/easyfix) FIX engine.

## `fix_str!`

Compile-time validated `&'static FixStr` literal. The macro checks that all
bytes are printable ASCII (`0x20..=0x7E`) and produces a zero-cost conversion —
no runtime validation. Works in `const` contexts and ordinary expressions.

```rust
use easyfix_macros::fix_str;

let begin_string = fix_str!("FIX.4.4");
let sender = fix_str!("SENDER");
```

Invalid input is rejected at compile time:

```rust,compile_fail
use easyfix_macros::fix_str;

let bad = fix_str!("hello\x01world"); // error: wrong byte found at position 5
```

## License

MIT
