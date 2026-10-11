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
    And the runtime restarts before the flush window ends
    Then after the flush window a spans segment for "acme" contains that span exactly once

  Scenario: Each metric kind is stored in its own table
    When the SDK exports one gauge and one sum metric
    Then a metrics_gauge segment and a metrics_sum segment are committed for "acme"

  Scenario: Tenants are isolated
    Given tenant "globex" holds a token with scope "telemetry:ingest"
    When "acme" exports a span named "acme-only"
    Then no segment for "globex" contains "acme-only"

  Scenario: A request without the ingest scope is rejected
    When a token with scope "query:read" exports a span
    Then the export fails with status 403 and nothing is stored

  Scenario: An oversized request is rejected without retry
    When the SDK exports a request larger than the ingest body limit
    Then the export fails with status 413
