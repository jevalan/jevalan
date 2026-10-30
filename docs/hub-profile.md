# Hub profile

One page: what a host and an implementation agree on to ask for a decision. It is a profile of standards that already
exist, not a new protocol.

| Layer | Standard | In this repository |
| --- | --- | --- |
| Transport to the host | MCP | `jevalan mcp` (see `docs/superpowers/specs/2026-10-28-mcp-server-design.md`) |
| Classifier contract | System One, `POST /v1/systemone` | the backends in `config.yaml` |
| Decision envelope | `jev-hub/0.1`: `status`, an opaque `decision`, `provider`, `request_id` | `jevalan-hub/src/envelope.rs` |
| Typing | JSON Schema, one pair per capability (`input_schema`, `decision_schema`) | `config/capabilities/schemas/` |
| Trace | OpenTelemetry GenAI conventions | the session export planned in the session-capture spec |

## What a capability publishes

`GET /v1/hub/capabilities` and its twin `GET /.well-known/jev-hub.json` return the same document. Each entry carries
`id` (`<namespace>/<file name>`), `digest`, `namespace`, `publisher`, `requires`, `version`, `description`, the schemas,
the provider's id and kind, and the default deadline. **The manifest declares needs; the consumer supplies the
binding:** the command, the URL and the variables that are actually injected never travel in what is published, and
`requires.env` lists names only.

`digest` is `sha256:<hex>` of the resolved contract (schemas inlined, `provider` block left out), over a canonical
serialization with sorted keys. Rebinding a capability to another provider does not change it; changing a schema, the
version or the deadline does. Pinning by digest therefore pins a contract.

## What an implementation must honour

1. `abstain` and `error` never produce a permission or an execution, and never carry a `decision`.
2. `abstain` is not `ok` with an empty decision.
3. An exhausted deadline is an `error` with a stable code (`deadline_exceeded`), never a default decision. A deadline
   asked by the caller can shorten the capability's, never lengthen it.
4. An incompatible version or an input or decision outside its schema is an `error`.
5. Neither the envelope nor the public events carry a command, an environment, a URL, the `input` or the `decision`.
6. Swapping a capability's provider by configuration does not change what the consumer receives.

## Checking it

`cargo test -p jevalan-daemon --test conformance` replays fixed cases against a stand-in specialist: no network, no paid
provider, and two runs give the same answers byte for byte once durations and trace ids are set aside. An
implementation that brings its own daemon meets the profile when those requests, sent to it, get the same answers.

A public index of manifests comes last, and only once someone else's implementation passes the cases above. It lives
outside this repository, in a project of its own that depends on what is published here (this profile, the `digest`,
`/.well-known/jev-hub.json` and the conformance cases) and never the other way round. Here only the capabilities that
serve as examples are kept: `tool.risk` (a third party's specialist, through `exec`), `agent.stop` and `agent.wakeup`
(native flows).
