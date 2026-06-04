# Interoperability

Flaps integrates through open standards only. The test for every integration point: a user with no other tooling from our ecosystem must get the full value of Flaps.

## Consumption: OpenFeature and OFREP

Flaps implements the OpenFeature Remote Evaluation Protocol (OFREP): `POST /ofrep/v1/evaluate/flags/{key}` for single evaluation and `POST /ofrep/v1/evaluate/flags` for bulk with ETag support. Any OpenFeature SDK with a generic OFREP provider works against Flaps with zero proprietary code.

## In-process: the flagd format

The compiled ruleset is flagd compatible. The `flaps-client` crate provides an OpenFeature in-process provider for Rust; in-process providers in other languages that consume the flagd format can evaluate Flaps rulesets too.

## Change notifications: SSE over plain HTTP

Ruleset change notifications use server-sent events with a notify-then-fetch contract. Clients without SSE support fall back to polling.

## Identity: any OIDC provider (v0.2)

Human authentication starts with local accounts. Generic OIDC discovery lands in v0.2 and works with any compliant identity provider.

## Federation: external references

`Project` and `Environment` carry two optional fields for embedding Flaps into a larger platform:

- `external_ref`: an opaque, indexed, unique URI set by an external system. Flaps never interprets it.
- `managed_by`: a display hint that warns before manual edits of externally managed resources.

The admin API supports idempotent upsert by `external_ref`, so an external control plane can reconcile desired state instead of scripting imperative calls. Identifiers cross the boundary; structures never do.

## No lock-in test

| Concern | Standard | Works with |
|---|---|---|
| Flag consumption | OpenFeature / OFREP | any OpenFeature SDK |
| In-process evaluation | flagd format | any flagd compatible provider |
| Human SSO (v0.2) | OIDC discovery | any compliant IdP |
| Platform embedding | opaque external_ref upsert | any control plane |
