# Boots the NixOS module (nix/nixos-module.nix) in VMs: `server` keeps the
# loopback default, `exposed` binds every interface with the firewall open.
# Checks readiness, the bind boundary, OTLP ingest and SQL, persistence across
# a restart, and the data directory's ownership.
{ testers, module }:
testers.runNixOSTest {
  name = "sigiro-nixos-module";

  defaults =
    { pkgs, ... }:
    {
      imports = [ module ];
      environment.systemPackages = [ pkgs.curl ];
    };

  nodes = {
    server = {
      services.sigiro = {
        enable = true;
        settings.SIGIRO_RETENTION_DAYS = 7;
      };
    };
    exposed = {
      services.sigiro = {
        enable = true;
        bind = "0.0.0.0";
        openFirewall = true;
      };
    };
  };

  testScript = ''
    import json
    import time

    start_all()
    for machine in (server, exposed):
        machine.wait_for_unit("sigiro.service")
        machine.wait_until_succeeds("sigiro healthcheck", timeout=120)

    with subtest("the default bind stays on loopback"):
        server.succeed("ss -ltnH | grep -q '127.0.0.1:9999'")
        server.fail("ss -ltnH | grep -E -q '(0.0.0.0|\\*|\\[::\\]):9999'")
        exposed.fail("curl --silent --fail --max-time 5 http://server:9999/health")

    with subtest("an explicit bind with openFirewall is reachable"):
        server.succeed("curl --silent --fail --max-time 5 http://exposed:9999/health")
        server.succeed("curl --silent --fail --max-time 5 -o /dev/null http://exposed:9999/dashboard/")

    with subtest("settings reach the service"):
        server.succeed("systemctl show sigiro -p Environment | grep -q SIGIRO_RETENTION_DAYS=7")

    def count():
        out = server.succeed(
            "curl --silent --show-error --fail -H 'Content-Type: text/plain' "
            "--data \"SELECT count(*) AS row_count FROM sigiro_logs WHERE service_name = 'nixos-module'\" "
            "http://127.0.0.1:9999/v1/query"
        )
        return json.loads(out)[0]["row_count"]

    with subtest("OTLP ingest is queryable and survives a restart"):
        now = time.time_ns()
        payload = json.dumps({"resourceLogs": [{
            "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "nixos-module"}}]},
            "scopeLogs": [{"logRecords": [{"timeUnixNano": str(now), "body": {"stringValue": "hello"}}]}],
        }]})
        server.succeed(f"curl --silent --show-error --fail -H 'Content-Type: application/json' --data '{payload}' http://127.0.0.1:4318/v1/logs")
        for _ in range(30):
            if count() == 1:
                break
            time.sleep(1)
        assert count() == 1, "the ingested log is not queryable"
        server.systemctl("restart sigiro.service")
        server.wait_until_succeeds("sigiro healthcheck", timeout=120)
        assert count() == 1, "the log did not survive a restart"

    with subtest("the data directory belongs to the service user"):
        server.succeed("test \"$(stat -c %U:%G:%a /var/lib/sigiro)\" = sigiro:sigiro:750")
        server.succeed("test -z \"$(find /var/lib/sigiro ! -user sigiro)\"")
  '';
}
