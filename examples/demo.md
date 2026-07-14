# Payment Service Design

This document exercises **every construct** markmaid supports — prose, ~~scrapped ideas~~, `inline code`, and a [runbook link](https://example.com/runbook).

## Main flow

```mermaid
flowchart LR
    User([User]) --> GW(API Gateway)
    GW --> Pay[Payment Service]
    Pay --> DB[(Postgres)]
    Pay -.->|async| Q[(Queue)]
```

## Transaction lifecycle

Every transaction is a small state machine:

```mermaid
stateDiagram-v2
    [*] --> Pending
    Pending --> Paid : capture ok
    Pending --> Failed : declined
    Failed --> Pending : retry
    Paid --> [*]
```

## Architecture snapshot

The current topology, exported nightly:

![payment topology diagram](https://example.com/topology.png)

## SLA per component

| Component | Target | Status |
|-----------|--------|--------|
| Gateway   | 99.9%  | **on-track** |
| Payment   | 99.95% | `at-risk` |
| Queue     | 99.5%  | on-track |

## Release checklist

- [x] load test at 2x traffic
- [x] runbook updated
- [ ] failover drill
- plain note without a checkbox
  1. nested ordered item
  2. another one

> Numbers come from the Q2 dashboard — *do not* reuse them for Q4 capacity.

## Traffic share

```mermaid
pie showData
    title Traffic per channel
    "Mobile" : 62
    "Web" : 31
    "API" : 7
```

```rust
// a plain code block stays a code block
fn main() { println!("hello"); }
```

---

That was a thematic break. The end.
