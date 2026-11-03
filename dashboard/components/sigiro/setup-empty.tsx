"use client";

import { IconPlugConnected } from "@tabler/icons-react";

import { Button } from "@/components/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";

const SETUP_GUIDE = "/docs/how-to/hosted-onboarding/";

/**
 * The stock shadcn Empty block, filled with what a first-time reader needs to
 * send data: the OTLP/HTTP endpoint, where the token comes from, and the guide.
 * The facts are the ones `www/docs/how-to/hosted-onboarding.md` states; keep
 * them in step with it.
 */
export function SetupEmpty({ range }: { range: string }) {
  return (
    <Empty className="border" data-state="setup">
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <IconPlugConnected />
        </EmptyMedia>
        <EmptyTitle>No telemetry in the {range}</EmptyTitle>
        <EmptyDescription>
          Point any OpenTelemetry SDK at sigiro over OTLP/HTTP. Your service
          shows up here a few seconds after its first export.
        </EmptyDescription>
      </EmptyHeader>
      <EmptyContent className="max-w-2xl text-left">
        <pre className="bg-muted w-full overflow-x-auto rounded-md p-3 text-xs">
          {`export OTEL_EXPORTER_OTLP_ENDPOINT="https://sigiro.com"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $(sigiro auth token)"
export OTEL_SERVICE_NAME="checkout"`}
        </pre>
        <ul className="text-muted-foreground grid list-disc gap-1 pl-5 text-sm">
          <li>
            The endpoint is <code>https://sigiro.com</code> with no port; the
            SDK adds <code>/v1/traces</code>, <code>/v1/logs</code> and{" "}
            <code>/v1/metrics</code>. gRPC is not available, so use HTTP.
          </li>
          <li>
            The token comes from <code>sigiro auth token</code> once the CLI is
            signed in (<code>curl -fsSL https://sigiro.com/install | sh</code>,
            then <code>sigiro auth login</code>). It lasts 15 minutes, which is
            enough to try sigiro; long-running services will use a per-project
            ingest key.
          </li>
        </ul>
        <Button asChild>
          <a href={SETUP_GUIDE}>Open the setup guide</a>
        </Button>
      </EmptyContent>
    </Empty>
  );
}
