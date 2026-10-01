Feature: DuckDB-WASM runtime gate for the Workers tenant lake
  Hosted Sigiro answers every query inside Cloudflare Workers (ADR-002), so the
  Workers DuckDB runtime must run Sigiro's own schema and documented SQL.

  Scenario: The runtime deploys within the Workers size limit
    Given the lake runtime Worker bundle
    When it is deployed to Cloudflare
    Then the deployment succeeds

  Scenario: Every documented SQL example runs on the Workers runtime
    Given a tenant lake created from Sigiro's table definitions
    And one span, one log and one gauge row with JSON attributes
    When each SQL example from the English docs is run
    Then every example returns rows without an error

  Scenario: A tenant lake catalog survives a restart
    Given a tenant lake with one committed span
    When its catalog is published and restored into a fresh database
    Then the restored lake returns the committed span
