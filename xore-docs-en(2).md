# Xore — Language & Compiler Documentation (EN)

> Xore is a small, statically-typed language with no loops and no exposed
> pointers, compiled directly to x86_64 and RISC-V64 machine code (no LLVM).
> This document describes the **actual current state** of the compiler,
> including what doesn't work yet. Unfinished features are clearly marked
> (⚠️ / ❌) instead of being glossed over.

## Table of contents

1. [Quick start](#quick-start)
2. [Source files](#source-files)
3. [Comments](#comments)
4. [Types](#types)
5. [Literals](#literals)
6. [Variables (`let`)](#variables-let)
7. [Functions](#functions)
8. [Calling functions](#calling-functions)
9. [The `if` expression](#the-if-expression)
10. [Operators](#operators)
11. [`include` and modules](#include-and-modules)
12. [Arrays](#arrays)
13. [No I/O (by design, for now)](#no-io-by-design-for-now)
14. [Bare metal / freestanding builds](#bare-metal--freestanding-builds)
15. [Known limitations and pitfalls](#known-limitations-and-pitfalls)
16. [CLI reference](#cli-reference)
17. [Example](#example)

---

## Quick start

```bash
cd xore
cargo build --release
cp target/release/xore_lang_new .

./xore_lang_new main.xre --target=x86_64
./main; echo $?
```

By default the compiler **produces a finished executable directly** — it
invokes the system assembler/linker (`cc`/`gcc`, or `riscv64-linux-gnu-gcc`
for RISC-V) internally. There's no manual linking step.

`main` in Xore maps to C's `main` — its `i32` becomes the process exit code,
exactly like in Go or Zig. This is currently the *only* way to observe a
program's result (see [Known limitations](#known-limitations-and-pitfalls) —
there's no I/O yet).

## Source files

| Extension | Role |
|---|---|
| `.xre` | source file — contains function implementations (`FnDef`) |
| `.xrh` | header file — usually declarations only (`FnDecl`, no body) |

The `.xre`/`.xrh` split is **purely conventional** (like `.c`/`.h`) — the
compiler loads both identically. The compiler's actual "linker" is
recursive expansion of `include "...";`, starting from the file given on the
command line — see [`include` and modules](#include-and-modules).

## Comments

Line comments only:

```xore
// this is a comment to end of line
let x: i32 = 5; // trailing comment
```

There are no block comments (`/* ... */`).

## Types

| Type | Description | Status |
|---|---|---|
| `i32`, `i64` | signed integers | ✅ fully working |
| `u32`, `u64` | unsigned integers | ⚠️ behave like `i32`/`i64` — see below |
| `f32`, `f64` | floating point | ❌ do not work correctly (see below) |
| `bool` | `True` / `False` | ✅ working |
| `Foo` (any name) | nominal type (future structs) | ❌ no struct definitions exist — unusable |
| `[T; N]` (arrays) | fixed-size arrays | ❌ parses but doesn't work at runtime |

**Why `u32`/`u64` and `f32`/`f64` don't fully work:** the internal IR does
not carry the type of an operation — `a + b` looks identical whether `a`/`b`
are ints or floats. Division/modulo always generate **signed** code, so for
`u32`/`u64` values above half the range the result will be wrong. Floating
point arithmetic isn't generated correctly at all — `f64`/`f32` literals are
currently stored as raw bit patterns in an integer register (the compiler
leaves a comment about this in the generated assembly), so any `+`/`*` on
them will produce nonsense. **Recommendation: stick to `i32`/`i64`/`bool`
until this is fixed in the compiler** (see the roadmap discussion with the
assistant).

## Literals

```xore
42          // IntLiteral (i32 by default)
0x1A        // hex literal
0b1010      // binary literal
0o17        // octal literal
3.14        // FloatLiteral — see limitations above
"text"      // StringLiteral
r"C:\path"  // RawStringLiteral (no \ escaping)
'a'         // CharLiteral
True        // Bool
False       // Bool
None        // "no value" — type Unknown
```

✅ **Hex/binary/octal literals (`0x1A`, `0b101`, `0o17`) work correctly.**
An earlier version of the lexer/lowering silently mis-parsed them as `0`;
this has been fixed and verified with a test comparing against Python's
evaluation of the same expression.

## Variables (`let`)

```xore
let x: i32 = 10;      // explicit type
let y = 20;             // inferred type
```

Variables are always internally mutable (there's no `const`/`mut` — the
checker always treats `let` as `is_mut: true`).

## Functions

```xore
// declaration (no body, ends with `;`) — usually in a .xrh file
public fn add(a: i32, b: i32) -> i32;

// definition (with body) — in a .xre file
public fn add(a: i32, b: i32) -> i32 {
    a + b;      // last expression-statement = implicit return value
}

fn helper(x: i32) -> i32 {   // no `public` = private (parses, but
    x * 2;                    // visibility is not enforced anywhere
}                              // today — see limitations)
```

**Important:** the return value is always the **last expression-statement**
of the function body (it must end with `;`, like everything in Xore). There
is no `return` keyword.

Parameter limit: **6 on x86_64, 8 on RISC-V64** (that's how many integer
argument registers the standard calling convention provides; passing extra
arguments on the stack isn't implemented yet — the compiler will fail
loudly with a clear error rather than silently generating wrong code).

## Calling functions

Xore does **not** use the classic `name(arguments)` syntax. Instead:

```xore
let result: i32 = add $ (10, 20);
//                ^^^^^^^^^^^^^^ call operator: `$` + parentheses
```

`add(10, 20)` (without `$`) will **not parse** — it will try to treat `add`
as a variable and `(10, 20)` as a separate, unrelated statement, and error
out. This is the most common mistake for anyone used to C-like syntax.

## The `if` expression

`if` in Xore is an **expression**, not a statement — it has a value:

```xore
let bigger: i32 = if a > b {
    a;
} else {
    b;
};   // <-- trailing semicolon, because the whole `if` is one expression-statement
```

A branch's value is its last expression-statement (same rule as function
bodies).

✅ Since this revision, an `if` used as a value with a missing/incomplete
`else` no longer reads stack garbage — the result slot is zero-initialized
before the branch runs, so a "half-formed" `if`-expression safely evaluates
to `0` instead of an undefined value. Still, prefer always writing an
explicit `else` when you use the value — it's clearer and avoids relying on
this fallback.

## Operators

| Operator | Meaning | Status |
|---|---|---|
| `+ - * / %` | arithmetic | ✅ fully working (int) |
| `== != < > <= >=` | comparisons (produce `bool`) | ✅ fully working |
| `$~` | `value $~ variable;` — store `value` into `variable` (must be an l-value) | ✅ working |
| `$ (...)` | function call | ✅ working (see above) |
| `->` | function return type | ✅ working |
| `:= ~= |= #== <=> => @ @/ @= -=` | reserved for future use | ❌ some don't even parse, others parse and pass type-checking but the compiler reports an `Unsupported operator` error during IR generation. **Do not use these in production code.** |

## `include` and modules

```xore
include "path/to/file.xre";
```

- The path is resolved **relative to the directory of the file containing
  the `include`** (not the current working directory).
- The compiler recursively visits every `include`, starting from the file
  passed on the command line, and collects all `FnDef` (bodied definitions)
  from every visited file into a single program. `FnDecl`s (declaration-only,
  as in `.xrh` files) generate no code — they just let a file parse/type-check,
  but **the actual implementation must exist in one of the `.xre` files that
  actually gets included**.
- If the same function is defined in two included files, the one found
  later wins (order follows `include` order).
- There is no cycle protection beyond deduplicating visited files (a file
  included twice is only loaded once).

## Arrays

Fixed-size arrays now have real memory behind them — this used to be a
complete stub (`[1,2,3]` always lowered to the constant `0`); it's been
implemented for real:

```xore
let arr: [i32; 5] = [10, 20, 30, 40, 50];
let i: i32 = 3;

arr[i];          // read at a runtime-computed index -> 40
99 $~ arr[i];    // write at a runtime-computed index (arr[3] = 99)
arr[2];          // read at a compile-time-constant index -> 30
```

- `[T; N]` is the array type: element type `T`, a fixed size `N` known at
  compile time.
- A constant index (`arr[2]`) compiles down to a plain, direct memory
  access at a fixed offset — it benefits from the same optimizations as any
  other local (store→load forwarding, dead-code elimination).
- A dynamic index (`arr[i]`) computes the address at runtime (`base -
  index * 8`, since elements grow "downward" in this compiler's stack
  layout convention) — implemented natively in both backends (x86_64:
  `lea`+`sub`+`mov`; RISC-V64: `li`+`sub`+`sub`+`ld`/`sd`, since RISC-V has
  no scaled-addressing instruction).
- Writing to an array element uses the same `$~` operator as everything
  else: `value $~ arr[index];`.
- **Whole-array copy** works: `let b = a;` (where `a` is a known local
  array) deep-copies all N elements into N new slots — verified to be a
  real copy, not an alias (mutating `b` afterward doesn't affect `a`).
- **Arrays can be passed to functions.** A local array used as a plain
  value (e.g. a call argument) decays to a pointer to its first element,
  exactly like in C:

  ```xore
  public fn sum3(arr: [i32; 3]) -> i32 {
      arr[0] + arr[1] + arr[2];   // indexes through the pointer
  }
  public fn main() -> i32 {
      let a: [i32; 3] = [10, 20, 30];
      sum3 $ (a);   // -> 60
  }
  ```

  Since this is pass-by-reference, a function **can** mutate the caller's
  array through `$~`, and the caller sees the change after the call
  returns — verified with a test where a callee zeroes `arr[0]` and the
  caller's subsequent read reflects it.

**Current scope (deliberately limited, documented rather than silently
broken):**
- **Returning an array from a function is rejected outright** at
  compile time with a clear error, rather than silently generating wrong
  code — it would need an `sret`-style ABI convention (caller allocates
  space, passes a hidden pointer) that doesn't exist yet.
- Indexing only works directly on a named array variable (`arr[i]`), not on
  an arbitrary expression (`f()[i]` isn't supported).
- No multi-dimensional arrays, no runtime bounds checking.
- Mixed-type array literals (`[1, True]`) are now correctly rejected by the
  type checker — this used to silently pass (a real bug fixed while adding
  this feature).

**A subtle aliasing bug found and fixed while adding function parameters:**
the store→load forwarding optimization didn't know that `LoadAddr` (the
instruction behind array-to-pointer decay) lets a local array's address
escape to another function. It was treating the array's initializing
stores as "dead" (never read *locally*) and deleting them — so `sum3(a)`
above returned `0` instead of `60` until this was fixed. The fix is
conservative but always correct: any function that takes the address of a
local array has this whole optimization pass disabled for it (a small,
scoped cost, only for functions that pass arrays by reference).

## No I/O (by design, for now)

Xore has no `print`, no file access, no way to read input. The only way to
observe a computed result is `main`'s returned `i32`, which becomes the
process exit code.

An earlier revision of this compiler had `print`/`println`/`exit` recognized
by name directly inside the compiler (`checker.rs`/`lowering.rs`), calling
hand-written syscall wrappers. **This was removed on purpose**: hardcoding
specific standard-library function names into the compiler's core is bad
architecture — it's a hidden dependency with no general mechanism behind
it (no `extern`/FFI declaration, just special-cased string matching on a
function's name). Real I/O will come back once there's a proper mechanism
for declaring external/intrinsic functions, not before.

## Bare metal / freestanding builds

Every Xore binary is built **without libc, without crt0/crt1, and without a
dynamic linker**. The compiler itself emits the process entry point
(`_start`), which calls your `main` and passes its return value straight to
the `exit` syscall:

```bash
./xore_lang_new main.xre --target=x86_64
ldd main        # -> "not a dynamic executable"
file main        # -> statically linked ELF executable
```

This is "bare metal" in the practical, buildable sense of **freestanding
userspace**: no libc dependency, pure raw syscalls, a minimal self-contained
static ELF binary — useful for embedded/minimal-container scenarios or
learning how a runtime is built from scratch. It is **not** a bootable OS
kernel target: Xore binaries still run as normal Linux processes started by
the kernel's ELF loader (`execve`), using Linux syscalls (`write`, `exit`)
for I/O and process control. Producing an actual boot-sector/kernel image
(no OS underneath at all, direct hardware/MMIO access, a custom linker
script, no syscalls) is a fundamentally different target and is not what
this compiler does today.

## Known limitations and pitfalls

- **No loops.** `while`/`for` are reserved lexer keywords, but the parser
  doesn't handle them — using them is a parse error. This is a deliberate
  design decision of Xore, not an oversight.
- **No I/O at all** (see [No I/O](#no-io-by-design-for-now) above) — the
  only observable result is `main`'s exit code.
- **Arrays have a limited scope** (see [Arrays](#arrays) above): no
  multi-dimensional arrays, no runtime bounds checking, no returning an
  array from a function (rejected outright, not silently broken).
- **`enum` is still a stub.** It parses and passes type-checking, but
  generates no code at all and has no way to reference a variant.
- **`public`/private visibility is not enforced.** It parses, but nothing
  currently checks whether a private function is called from outside its
  module.
- **No floating point arithmetic and no correct unsigned semantics** — see
  the [Types](#types) section.

## CLI reference

```
xore_lang_new <file.xre> [options]
```

| Flag | Description |
|---|---|
| `--target=x86_64` \| `--target=riscv64` | target architecture (default `x86_64`) |
| `-o <file>` | output path (default: source filename without extension for a binary, `<name>.<arch>.s` for `--emit-asm`) |
| `--emit-asm` | stop at the textual `.s` file, don't link a binary |
| `--keep-asm` | when building a binary, also keep the generated `.s` alongside it |
| `--no-opt` | disable IR optimizations (constant folding, DCE, memory forwarding...) |
| `--dump-ir` | print the generated IR (after optimizations) to stderr — useful for debugging |
| `-h`, `--help` | help |

**Note on RISC-V:** compiling to RISC-V64 requires a cross-toolchain
(`riscv64-linux-gnu-gcc`) installed on the machine running
`xore_lang_new` (e.g. `apt install gcc-riscv64-linux-gnu
binutils-riscv64-linux-gnu`). If it's missing, the compiler will say so
clearly and leave the generated `.s` on disk instead of pretending success.
Running RISC-V binaries on an x86_64 machine requires an emulator, e.g.
`qemu-riscv64`.

## Example

```xore
// mathutil.xre
public fn maxi(a: i32, b: i32) -> i32 {
    if a > b {
        a;
    } else {
        b;
    };
}

// main.xre
include "mathutil.xre";

public fn main() -> i32 {
    let values: [i32; 3] = [7, 19, 12];
    let biggest: i32 = maxi $ (maxi $ (values[0], values[1]), values[2]);
    biggest;   // -> process exit code: 19
}
```

```bash
./xore_lang_new main.xre --target=x86_64
./main; echo $?   # 19
```
