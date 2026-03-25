# Error Handling in Rust

## Introduction

Rust's error handling is built around the `Result<T, E>` type.
Unlike exceptions in other languages, **Rust enforces error handling at compile time**.

## Basic Patterns

### The `?` Operator

The most common pattern for propagating errors:

```rust
fn read_config(path: &str) -> Result<Config, Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}
```

### Custom Error Types

| Crate | Feature | Use Case |
|-------|---------|----------|
| `thiserror` | derive macro | Libraries |
| `anyhow` | dynamic error type | Applications |
| `eyre` | custom reports | CLI tools |

> **Note**: `thiserror` is the standard choice for libraries,
> while `anyhow` is preferred for applications.
>
> > However, this is not an absolute rule.

## Error Handling Steps

1. Define your error type
2. Return `Result<T, E>`
3. Use the `?` operator at call sites

Anti-patterns to avoid:

- Excessive use of `unwrap()`
- Silently swallowing errors
- ~~Using `panic!` for control flow~~ (not recommended outside tests)

## Inline Math

Euler's identity $e^{i\pi} + 1 = 0$ is considered the most beautiful equation in mathematics.

The quadratic formula gives $x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}$.

## Display Math

Taylor expansion:

$$f(x) = \sum_{n=0}^{\infty} \frac{f^{(n)}(a)}{n!}(x-a)^n$$

Gaussian integral:

$$\int_{-\infty}^{\infty} e^{-x^2} \, dx = \sqrt{\pi}$$

## Combined Usage

The following list combines **inline styles**, `code`, and $E = mc^2$:

1. **Bold** item
2. Item with `code`
3. Item with math $\sum_{i=1}^{n} i = \frac{n(n+1)}{2}$

> *Italic* text in a blockquote with $\alpha + \beta = \gamma$.

## Special Characters

Typst special characters: $100 price tag, #hashtag, @mention.
These should be escaped and displayed as-is.

---

*This document verifies all supported features with Latin-only content.*
