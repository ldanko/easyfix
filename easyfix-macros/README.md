# easyfix-macros

Proc macros for the [easyfix](https://github.com/ldanko/easyfix) FIX engine.

## `fix_str!`

Compile-time validated `&'static FixStr` literal. The macro checks that all
bytes are printable ASCII (0x20..0x7F) and produces a zero-cost conversion —
no runtime validation.

```rust
use easyfix_core::fix_str;

let begin_string = fix_str!("FIX.4.4");
let sender = fix_str!("SENDER");
```

Invalid input is rejected at compile time:

```rust,compile_fail
let bad = fix_str!("hello\x01world"); // error: wrong byte found at position 5
```

## License

MIT
