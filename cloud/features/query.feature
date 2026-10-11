Feature: Hosted telemetry query

  Background:
    Given a hosted Sigiro running under celld dev
    And tenant "acme" exported spans "POST /checkout" twice and "GET /cart" once for service "checkout"
    And tenant "acme" exported one log for service "worker"
    And tenant "globex" exported one span "globex-only" for service "billing"
    And every export has been flushed

  Scenario: A query returns one JSON object per row
    When "acme" queries "SELECT span_name, count(*) AS n FROM sigiro_spans GROUP BY 1 ORDER BY 1" with scope "query:read"
    Then the response is 200 with body [{"span_name":"GET /cart","n":1},{"span_name":"POST /checkout","n":2}]
    And the header "x-sigiro-truncated" is "false"

  Scenario: The hosted result equals DuckDB over the same segments
    When "acme" queries "SELECT service_name, count(*) AS n FROM sigiro_spans GROUP BY 1"
    Then the rows equal the duckdb CLI running that query over acme's committed spans segments

  Scenario: A tenant sees only its own rows
    When "globex" queries "SELECT span_name FROM sigiro_spans"
    Then the response body is [{"span_name":"globex-only"}]

  Scenario Outline: Unsafe statements are refused before execution
    When "acme" queries "<sql>"
    Then the response is 400 with title "query rejected"

    Examples:
      | sql                                     |
      | DELETE FROM sigiro_spans                |
      | ATTACH 'x.db' AS x                      |
      | SELECT * FROM read_parquet('r2://x/y')  |
      | SELECT table_name FROM information_schema.tables |

  Scenario: A query without the query scope is rejected
    When "acme" queries "SELECT 1 AS one FROM sigiro_spans" with scope "telemetry:ingest"
    Then the response is 403

  Scenario: An over-memory query fails with a typed error and the next query succeeds
    When "acme" runs a query that exceeds the query memory limit
    Then the response is 422 with title "query exceeded memory limit"
    And the next query "SELECT count(*) AS n FROM sigiro_spans" returns [{"n":3}]

  Scenario: Services lists every service that reported telemetry
    When "acme" requests /v1/services with scope "services:read"
    Then the services are "checkout" with 3 spans and "worker" with 1 log, in that order
