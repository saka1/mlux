// mitex compatibility shims
//
// mitex::convert_math (Rust crate) outputs references to functions defined in
// the mitex Typst package (@preview/mitex). Since we only use the Rust crate,
// we must define these functions ourselves.

// Matrix environments
#let matrix(..args) = math.mat(..args)
#let pmatrix(..args) = math.mat(delim: "(", ..args)
#let bmatrix(..args) = math.mat(delim: "[", ..args)
#let vmatrix(..args) = math.mat(delim: "|", ..args)
#let Vmatrix(..args) = math.mat(delim: "‖", ..args)

// sqrt: mitexsqrt(x) or mitexsqrt([n], x) for nth root
#let mitexsqrt(..args) = {
  let a = args.pos()
  if a.len() == 1 { math.sqrt(a.at(0)) }
  else if a.len() >= 2 { math.root(a.at(0), a.at(1)) }
}

// Bold math
#let mitexmathbf(it) = math.bold(math.upright(it))

// Operator name
#let operatorname(..args) = math.op(args.pos().join())

// Text in math (used as #textmath[...] by mitex)
#let textmath(it) = it

// Display style
#let mitexdisplay(..args) = math.display(args.pos().join())

// Decorations
#let mitexoverbrace(it) = math.overbrace(it)
#let mitexunderbrace(it) = math.underbrace(it)

// Array environment (basic: treat like matrix)
#let mitexarray(..args) = math.mat(..args)

// pmod: renders as (mod p)
#let pmod(it) = $\(mod it\)$
