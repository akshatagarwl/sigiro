Feature: Hosted telemetry ingest

  Background:
    Given a hosted Sigiro running under celld dev
    And tenant "acme" holds a token with scope "telemetry:ingest"

  Scenario: An acknowledged span is committed as a segment
    When the SDK exports a span named "POST /checkout" for service "checkout"
    Then the export succeeds
    And within the flush window a spans segment for "acme" contains that span

  Scenario: An acknowledged span survives an ingest cell restart before its flush
    When the SDK exports a span named "POST /checkout" and the export succeeds
    And no spans segment contains that span before the runtime stops
    And the runtime restarts before the flush window ends
    Then after the flush window all spans segments for "acme" contain that span exactly once

  Scenario: Each metric kind is stored in its own table
    When the SDK exports one gauge and one sum metric
    Then a metrics_gauge segment and a metrics_sum segment are committed for "acme"

  Scenario: Tenants are isolated
    Given tenant "globex" holds a token with scope "telemetry:ingest"
    When "acme" exports a span named "acme-only"
    And "globex" exports a span named "globex-only"
    Then segments for "acme" contain "acme-only" and not "globex-only"
    And segments for "globex" contain "globex-only" and not "acme-only"

  Scenario: A token with a malformed tenant is rejected
    When a token for tenant "../globex" exports a span
    Then the export fails with status 401 and no escaped tenant object is stored

  Scenario: A request without the ingest scope is rejected
    When a token with scope "query:read" exports a span
    Then the export fails with status 403 and nothing is stored

  Scenario: An oversized request is rejected without retry
    When the SDK exports a request larger than the ingest body limit
    Then the export fails with status 413

  Scenario: Ingest during an ingest cell takeover is retryable
    Given the node owning "acme"'s spans cell crashed
    When the SDK exports a span before another node has taken the cell over
    Then the export fails with status 503 and a Retry-After header
    And a retried export succeeds once the cell is taken over

  Scenario: A gzip-encoded export is accepted
    When the SDK exports a span with Content-Encoding gzip
    Then the span is committed in a spans segment

  Scenario: An OTLP profile is stored with root-first stacks
    When the SDK exports an OTLP profile whose leaf frame is "work" under "main"
    Then a profiles segment holds the stack "main;work" with its sample value

  Scenario: Profiles in a format other than OTLP are not accepted
    When a client posts a profile to /ingest
    Then the export fails with status 404
