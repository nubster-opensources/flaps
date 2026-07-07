# Conformance corpus

This directory contains the golden test cases for the `flaps-eval` engine.
Each `.json` file is one test case. The runner in `tests/corpus.rs` discovers
all files recursively (sorted by path for reproducibility), evaluates them
through the public `flaps-eval` API, and reports every failure in one pass.

## Case format

```json
{
  "name": "unique_snake_case_identifier",
  "description": "Human-readable rationale, including the oracle derivation.",
  "flagd": {
    "flags": { ... }
  },
  "flagKey": "the_flag_to_evaluate",
  "context": {
    "targetingKey": "optional string",
    "attributes": {},
    "timestamp": 0
  },
  "expected": {
    "variant": "variant_key",
    "reason": "TARGETING_MATCH",
    "value": "variant_value"
  }
}
```

For error cases, replace `expected` with `expectedError`:

```json
{
  ...
  "expectedError": "FLAG_NOT_FOUND"
}
```

Valid `expectedError` codes: `FLAG_NOT_FOUND`, `VARIANT_NOT_FOUND`, `PARSE_ERROR`.

Valid `reason` strings (OFREP): `STATIC`, `TARGETING_MATCH`, `DEFAULT`, `DISABLED`.

## Oracle guarantee

Every `expected` value is derived from an **external oracle**, never from
running `flaps-eval` and copying its output (that would be a tautology).

- **Fractional buckets**: derived from the public MurmurHash3 x86-32 seed-0
  algorithm (`murmur3("", 0) = 0`, `murmur3("hello", 0) = 0x248bfa47`,
  `murmur3("headerColoruser:abc", 0) = 1681433475 -> bucket 39/100`,
  `murmur3("headerColorsquarey@example.com", 0) = 3228116633 -> bucket 75/100`).
  Bucket formula: `(hash as u64 * total_weight as u64) >> 32`.
- **JsonLogic**: derived from the JsonLogic specification
  (https://jsonlogic.com) and the flagd targeting schema.
- **Reasons and errors**: derived from the flagd flag definition schema v0
  and the OFREP specification.

If a case reveals that `flaps-eval` diverges from the oracle, that is a bug
in the engine. The `expected` value is never modified to match the engine;
instead a `bug(eval):` issue is opened and the case is documented.

## Directory layout

| Directory      | What it covers                                              |
|----------------|-------------------------------------------------------------|
| `fractional/`  | MurmurHash3 reference vectors, 50/50 splits, 0/100 splits, `bucket_by` override |
| `jsonlogic/`   | `==`, `!=`, coercion, `&&`, `||`, `!`, `if`/`else`, `var`  |
| `targeting/`   | `starts_with`, `ends_with`, `sem_ver`, `$evaluators` inlining |
| `resolution/`  | Reasons: Static, TargetingMatch, Default, Disabled; value presence |
| `errors/`      | Missing flag, unknown variant, invalid document             |
