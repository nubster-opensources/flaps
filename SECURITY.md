# Security policy

## Supported versions

Flaps follows the [semver policy](docs/SEMVER_POLICY.md). During the 0.x phase, only the latest minor release receives security fixes.

| Version | Supported          |
| ------- | ------------------ |
| 0.x latest | :white_check_mark: |
| older 0.x  | :x:                |

The supported window will be widened once Flaps reaches 1.0.

## Threat model

Flaps is a feature-flag evaluation library and server. Its threat model covers the evaluation engine, the store layer, the OFREP-compatible API and the compiler pipeline.

The following attack surfaces are explicitly in scope:

- **OFREP evaluation API** (`/v1/`), including authorization bypass and uncontrolled memory growth on large flag payloads.
- **Admin REST API and its authentication**, including privilege escalation, insecure direct object references and unauthenticated access to management endpoints.
- **SDK key handling**, including keys that must be stored hashed and revocation that must take effect immediately.
- **Flag evaluation engine** (`flaps-eval`), including rule injection through crafted flag definitions, targeting rule bypass and unbounded recursion in nested flag references.
- **Compiler pipeline** (`flaps-compiler`), including malformed input causing panics or unbounded memory allocation.
- **SQL store** (`flaps-store`), including SQL injection vectors and unauthorised data access through forged identifiers.

The following are assumed safe by the threat model:

- The PostgreSQL database and the local filesystem on the host running Flaps are trusted.
- Operators with shell access to the host running Flaps are trusted.
- The Rust standard library, `tokio`, `rustls`, `sqlx` and other supply-chain dependencies are trusted up to the vulnerabilities published in their respective advisories.

## Reporting a vulnerability

If you find a security vulnerability in Flaps, please **do not** open a public issue. Disclosure rules:

1. Email a detailed report to **security@nubster.com** with the subject prefix `[flaps security]`.
2. The report should include:
   - A description of the vulnerability and the attacker model.
   - Affected versions and crates.
   - Reproduction steps or a proof of concept.
   - The impact you anticipate (data leak, denial of service, privilege escalation, tenant isolation bypass, etc.).
   - Suggested mitigation if you have one.
3. You will receive an acknowledgement within **7 calendar days**. If you do not, please follow up at the same address.
4. We will work with you to validate, scope and remediate the issue. A coordinated disclosure timeline will be agreed in writing. The default embargo period is **90 days** from acknowledgement.
5. Once a fix is published, you will be credited in the release notes unless you prefer to remain anonymous.

## Encrypted reporting

If your report includes confidential proof-of-concept material, please encrypt it with the Nubster security GPG key. The fingerprint and public key are published at <https://nubster.com/.well-known/security.txt> (once Nubster publishes them).

## Out of scope

The following are explicitly **out of scope** for vulnerability reports:

- Issues in unsupported versions.
- Vulnerabilities in third-party dependencies that are already publicly disclosed and tracked upstream. Report them to the upstream project.
- Reports based on theoretical attacks without a working proof of concept.
- Misconfiguration by the operator (missing TLS certificate, world-readable key material on the filesystem). These are documented hazards, not vulnerabilities.
- Reports requiring an attacker already in possession of valid administrative credentials.
- Denial of service achievable only by malicious operators of the database the server connects to. The threat model assumes trusted infrastructure.

## Public security advisories

Confirmed and fixed vulnerabilities are published on the repository Security Advisories page. RustSec advisories are also coordinated for severe issues when applicable.
