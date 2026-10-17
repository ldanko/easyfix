# easyfix-dictionary

Parses FIX XML specifications into Rust data structures for inspection and code
generation.

Part of the [easyfix](https://github.com/ldanko/easyfix) FIX engine.

## Overview

The dictionary represents the FIX protocol structure:

- **Fields** — data elements with types and possible enumerated values
- **Components** — reusable groups of fields
- **Groups** — repeating sets of fields or components
- **Messages** — message types composed of fields, components, and groups

## Usage

`DictionaryBuilder` loads XML files into a `Dictionary`. For FIX 4.x, provide
a single FIX XML file. For FIX 5.0+, combine a FIXT XML file with application
XML files and access each application dictionary through `subdictionary()`.

The builder offers optional strict validation. `flatten()` inlines components
into messages and groups for simpler traversal and code generation.

## License

MIT
