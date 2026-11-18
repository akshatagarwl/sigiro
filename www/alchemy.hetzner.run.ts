import * as CloudflareState from "@alchemy.run/cloudflare/StateStore";
import * as Hetzner from "@alchemy.run/hetzner";
import * as Alchemy from "alchemy";
import { adopt } from "alchemy/AdoptPolicy";
import { retain } from "alchemy/RemovalPolicy";
import * as Effect from "effect/Effect";

const publicIngress = ["0.0.0.0/0", "::/0"];

export default Alchemy.Stack(
  "LinguabenchInfrastructure",
  {
    providers: Hetzner.providers(),
    state: CloudflareState.state(),
  },
  Effect.gen(function* () {
    const sshKey = yield* Hetzner.SshKey("SshKey", {
      name: "linguabench",
      publicKey:
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICMLzSP6E7HnsEe/iZ6fhZiAIxWpcI+IKw6vOHlEDtug",
    }).pipe(adopt(true), retain());
    const ipv4 = yield* Hetzner.PrimaryIp("Ipv4", {
      name: "linguabench-ipv4",
      type: "ipv4",
      location: "nbg1",
      autoDelete: false,
      deleteProtection: true,
    }).pipe(adopt(true), retain());
    const ipv6 = yield* Hetzner.PrimaryIp("Ipv6", {
      name: "primary_ip-137074465",
      type: "ipv6",
      location: "nbg1",
      autoDelete: true,
      deleteProtection: false,
    }).pipe(adopt(true), retain());
    const server = yield* Hetzner.Server("Server", {
      name: "sigiro-linguabench",
      serverType: "cx23",
      image: "docker-ce",
      location: "nbg1",
      sshKeys: [sshKey],
      deleteProtection: true,
    }).pipe(adopt(true), retain());
    const firewall = yield* Hetzner.Firewall("Firewall", {
      name: "linguabench-fw",
      applyTo: [server],
      rules: [
        {
          direction: "in",
          protocol: "tcp",
          port: "22",
          sourceIps: publicIngress,
        },
        {
          direction: "in",
          protocol: "tcp",
          port: "80",
          sourceIps: publicIngress,
        },
        {
          direction: "in",
          protocol: "tcp",
          port: "443",
          sourceIps: publicIngress,
        },
      ],
    }).pipe(adopt(true), retain());

    return {
      serverId: server.serverId,
      serverIpv4: server.ipv4,
      sshKeyId: sshKey.id,
      firewallId: firewall.id,
      ipv4Id: ipv4.id,
      ipv6Id: ipv6.id,
    };
  })
);
