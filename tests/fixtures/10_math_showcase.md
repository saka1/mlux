# Math Showcase

A collection of formulas from various fields of mathematics.

## Linear Algebra

The eigenvalue problem $\det(A - \lambda I) = 0$ is called the characteristic equation.

The determinant:

$$\det(A) = \sum_{\sigma \in S_n} \varepsilon(\sigma) \prod_{i=1}^{n} a_{i,\sigma(i)}$$

The Cauchy-Schwarz inequality:

$$\left| \sum_{i=1}^{n} u_i v_i \right|^2 \leq \left( \sum_{i=1}^{n} u_i^2 \right) \left( \sum_{i=1}^{n} v_i^2 \right)$$

For eigenvalues $\lambda_1, \ldots, \lambda_n$, we have $\lambda_1 + \cdots + \lambda_n = a_{11} + \cdots + a_{nn}$ and $\det(A) = \prod_{i=1}^{n} \lambda_i$.

## Calculus

The Leibniz integral rule:

$$\frac{d}{dx} \int_{a(x)}^{b(x)} f(x, t) \, dt = f(x, b(x)) b'(x) - f(x, a(x)) a'(x) + \int_{a(x)}^{b(x)} \frac{\partial}{\partial x} f(x, t) \, dt$$

L'Hopital's rule: when $\lim_{x \to c} \frac{f(x)}{g(x)}$ is of the form $\frac{0}{0}$:

$$\lim_{x \to c} \frac{f(x)}{g(x)} = \lim_{x \to c} \frac{f'(x)}{g'(x)}$$

A double integral using the polar area element $dA = r \, dr \, d\theta$:

$$\iint_D f(x, y) \, dA = \int_0^{2\pi} \int_0^R f(r\cos\theta, r\sin\theta) \, r \, dr \, d\theta$$

Green's theorem:

$$\oint_C (P \, dx + Q \, dy) = \iint_D \left( \frac{\partial Q}{\partial x} - \frac{\partial P}{\partial y} \right) dA$$

## Infinite Series

The Basel problem:

$$\sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}$$

The Leibniz series:

$$\frac{\pi}{4} = \sum_{k=0}^{\infty} \frac{(-1)^k}{2k+1} = 1 - \frac{1}{3} + \frac{1}{5} - \frac{1}{7} + \cdots$$

Maclaurin expansions:

$$e^x = \sum_{n=0}^{\infty} \frac{x^n}{n!}, \quad \sin x = \sum_{n=0}^{\infty} \frac{(-1)^n x^{2n+1}}{(2n+1)!}$$

The Euler product for the Riemann zeta function $\zeta(s) = \sum_{n=1}^{\infty} n^{-s}$:

$$\zeta(s) = \prod_{p} \frac{1}{1 - p^{-s}}$$

## Physics

Maxwell's equations:

$$\nabla \cdot E = \frac{\rho}{\varepsilon_0}, \quad \nabla \times B = \mu_0 J + \mu_0 \varepsilon_0 \frac{\partial E}{\partial t}$$

The Schrodinger equation:

$$i\hbar \frac{\partial}{\partial t} \Psi(r, t) = \left[ -\frac{\hbar^2}{2m} \nabla^2 + V(r, t) \right] \Psi(r, t)$$

Einstein's field equations:

$$R_{\mu\nu} - \frac{1}{2} R g_{\mu\nu} + \Lambda g_{\mu\nu} = \frac{8\pi G}{c^4} T_{\mu\nu}$$

The partition function for the Boltzmann distribution $P(E) = \frac{1}{Z} e^{-E / k_B T}$:

$$Z = \sum_i e^{-E_i / k_B T}$$

## Probability and Statistics

Bayes' theorem:

$$P(A \mid B) = \frac{P(B \mid A) \, P(A)}{P(B)}$$

The probability density function of the normal distribution:

$$f(x) = \frac{1}{\sigma (2\pi)^{1/2}} \exp\left( -\frac{(x - \mu)^2}{2\sigma^2} \right)$$

The law of large numbers: for i.i.d. random variables $X_1, X_2, \ldots$:

$$\bar{X}_n = \frac{1}{n} \sum_{i=1}^{n} X_i \to \mu \quad (n \to \infty)$$

## Number Theory

Euler's formula:

$$e^{i\theta} = \cos\theta + i\sin\theta$$

Fermat's little theorem: for a prime $p$ and $\gcd(a, p) = 1$, we have $a^{p-1} \equiv 1 \mod p$.

The law of quadratic reciprocity (Legendre symbol):

$$\left( \frac{p}{q} \right) \left( \frac{q}{p} \right) = (-1)^{\frac{p-1}{2} \cdot \frac{q-1}{2}}$$

The prime number theorem: $\pi(x) \sim \frac{x}{\ln x}$, that is, $\lim_{x \to \infty} \frac{\pi(x) \ln x}{x} = 1$.

## Matrices

Matrix delimiter variations:

$$\begin{pmatrix} a & b \\ c & d \end{pmatrix}, \quad \begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}, \quad \begin{vmatrix} a & b \\ c & d \end{vmatrix}$$

## Roots, Operators, and Text

The Euclidean norm $\sqrt{x^2 + y^2}$ and a cube root $\sqrt[3]{27}$.

The sign function $\operatorname{sgn}(x)$ and a bold vector $\mathbf{v} = (v_1, v_2)$.

A piecewise definition:

$$f(x) = \begin{cases} 1 & \text{if } x > 0 \\ 0 & \text{otherwise} \end{cases}$$

## Dense Inline Math

A function $f: \mathbb{R} \to \mathbb{R}$ is differentiable at a point $x_0$ if the limit $f'(x_0) = \lim_{h \to 0} \frac{f(x_0 + h) - f(x_0)}{h}$ exists. In that case, $f$ is continuous at $x_0$, and the slope of the tangent line is given by $\tan\alpha = f'(x_0)$. In particular, if $f(x) = x^n$, then $f'(x) = nx^{n-1}$.
