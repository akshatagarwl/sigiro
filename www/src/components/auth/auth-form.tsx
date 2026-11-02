import { useCallback, useEffect, useRef, useState } from "react";

import { CopySource } from "@/components/site/copy-source";
import { buttonVariants } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  InputOTP,
  InputOTPGroup,
  InputOTPSlot,
} from "@/components/ui/input-otp";
import { Label } from "@/components/ui/label";
import { INSTALL_COMMAND } from "@/lib/site.shared";

/**
 * The entire auth card: email, one-time code, device approval, consent, done.
 *
 * It is one component on purpose. The first port of these pages kept a vanilla
 * `auth-ui.ts` module driving hidden `<div>`s and bolted a React field on top,
 * which left two owners for one flow — the island holding the email and code,
 * the module deciding what happened after. Phase is state; this owns it.
 *
 * Built from stock shadcn/ui components (`src/components/ui`). The code cells
 * share one real input through `input-otp`, so rapid typing,
 * paste, mobile autofill, password managers, undo and screen readers all use
 * the browser's native text-field behavior instead of focus-shuffling handlers.
 *
 * IDs are load-bearing: `check-signup-browser.ts` and `check-device-flow.ts`
 * drive production by selector, so `#email-form`, `#otp-form`, `#approve` and
 * `input[name="email"]` survive the rewrite. The verification input is named
 * `verification-code`; the checks fill that one native field just as autofill does.
 */

export type AuthPage =
  | "sign-in"
  | "sign-up"
  | "device"
  | "agent-device"
  | "consent";

interface Props {
  readonly page: AuthPage;
  /**
   * The heading for the page's FIRST step, server-rendered by the shell so the
   * document has a real `<h1>` before hydration. Later steps replace the intro
   * from here: leaving "Enter your email" above six code cells is the kind of
   * copy that reads as broken.
   */
  readonly title: string;
  readonly intro: string;
}

type Phase =
  | "loading"
  | "email"
  | "code"
  | "request"
  | "workspace"
  | "consent"
  | "done";

/** One row of better-auth's `/organization/list`. */
interface OrganizationRow {
  readonly id: string;
  readonly name: string;
  readonly slug?: string | null;
}

const api = "/api/auth";
const resendCooldownSeconds = 30;
const LOGIN_COMMAND = "sigiro auth login" as const;

interface ErrorBody {
  readonly message?: string;
  readonly error_description?: string;
  readonly error?: string;
}

interface SessionBody {
  readonly session?: unknown;
  readonly user?: { readonly email?: string };
}

interface WorkspaceBody {
  readonly organization: { readonly id: string; readonly name: string };
  readonly created: boolean;
}

interface Workspace {
  readonly id: string;
  readonly name: string;
  readonly role: string;
  readonly personal: boolean;
}

interface DeviceWorkspaces {
  readonly client_name: string | null;
  readonly expires_at: string;
  readonly workspaces: readonly Workspace[];
  readonly suggested: string | null;
}

/** Plain-language descriptions for the scopes a device request can carry. */
const SCOPE_WORDS = new Map([
  ["openid", "Know who you are"],
  ["profile", "Know who you are"],
  ["email", "Know who you are"],
  ["offline_access", "Stay signed in until you log out"],
  ["telemetry:ingest", "Send telemetry"],
  ["services:read", "Read services and traces"],
  ["traces:read", "Read services and traces"],
  ["query:read", "Run SQL queries"],
  ["alerts:read", "Read alerts"],
  ["alerts:write", "Change alerts"],
  ["tenant:members", "Manage members"],
  ["tenant:agents", "Manage agents"],
  ["tenant:admin", "Administer the workspace"],
  ["optimizer:write", "Run the optimizer"],
]);

const describeScopes = (scope: string | undefined): string => {
  const words = new Set(
    (scope ?? "")
      .split(/\s+/)
      .filter(Boolean)
      .map((name) => SCOPE_WORDS.get(name) ?? name)
  );
  return words.size > 0 ? [...words].join(" · ") : "No access requested";
};

/**
 * The signed authorization request this page was opened with, in the form
 * `/oauth2/continue` and `/oauth2/consent` verify. The OAuth provider redirects
 * here with its signed parameters spread over the URL (`…&ba_param=…&sig=…`),
 * so keep exactly the parameters it signed, as `buildSignedOAuthQuery` in
 * `@better-auth/oauth-provider` does; anything else would fail the signature.
 */
const signedOAuthQuery = (params: URLSearchParams): string | null => {
  const wrapped = params.get("oauth_query");
  if (wrapped) return wrapped;
  if (!params.has("sig")) return null;
  const names = new Set(params.getAll("ba_param"));
  if (names.size === 0) return null;
  const signed = new URLSearchParams();
  for (const [key, value] of params.entries())
    if (key === "sig" || key === "ba_param" || names.has(key))
      signed.append(key, value);
  return signed.toString();
};

/** `/oauth2/continue` and `/oauth2/consent` answer `{ redirect: true, url }`. */
interface RedirectBody {
  readonly url: string;
}

/** The body of a device or agent approval decision. */
type DecisionBody =
  | { readonly userCode: string; readonly organizationId?: string }
  | {
      readonly agent_id: string;
      readonly user_code: string;
      readonly action: "approve" | "deny";
    };

interface DeviceDetails {
  readonly client_id?: string;
  readonly scope?: string;
  readonly resource?: string | readonly string[];
}

const request = async <T,>(
  path: string,
  options: RequestInit = {}
): Promise<T> => {
  const response = await fetch(api + path, {
    credentials: "same-origin",
    ...options,
    headers:
      options.body === undefined
        ? options.headers
        : { "content-type": "application/json", ...options.headers },
  });
  // SAFETY: every auth endpoint answers JSON; an unparseable body becomes an
  // empty object and surfaces as the generic failure below.
  const body = (await response.json().catch(() => ({}))) as T & ErrorBody;
  if (!response.ok) {
    throw new Error(
      body.message ?? body.error_description ?? body.error ?? "Request failed"
    );
  }
  return body;
};

const message = (cause: unknown, fallback: string): string =>
  cause instanceof Error ? cause.message : fallback;

const verificationMessage = (cause: unknown): string => {
  const detail = message(cause, "Verification failed");
  if (/expired/i.test(detail))
    return "That verification code has expired. Request a new code.";
  if (/invalid otp|invalid.*code/i.test(detail)) {
    return "That verification code is incorrect. Try again or request a new code.";
  }
  return detail.replace(/\bOTP\b/gi, "verification code");
};

const primary = buttonVariants({ size: "lg" });

const secondary = buttonVariants({ variant: "outline", size: "lg" });

export function AuthCard({ page, title, intro }: Props): React.ReactElement {
  /**
   * `sign-in` and `sign-up` start on the email step so the SERVER renders a
   * real form. Starting every page at `loading` emitted a card containing
   * nothing but `aria-busy`, which meant no markup without JavaScript and an
   * empty card for the first paint — worse than the hand-rolled UI this
   * replaced, which shipped the form and toggled `hidden`.
   *
   * The CLI-initiated pages keep `loading`: they are reached from a terminal
   * mid-flow, the reader is usually already signed in, and showing an email
   * form for a frame before replacing it with an approval prompt is the worse
   * trade there.
   */
  const [phase, setPhase] = useState<Phase>(
    page === "sign-in" || page === "sign-up" ? "email" : "loading"
  );
  const [email, setEmail] = useState("");
  const [name, setName] = useState("");
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [success, setSuccess] = useState("");
  const [details, setDetails] = useState<DeviceDetails | null>(null);
  const [account, setAccount] = useState("");
  const [workspace, setWorkspace] = useState<WorkspaceBody | null>(null);
  const [grantable, setGrantable] = useState<DeviceWorkspaces | null>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  const [oauthWorkspaces, setOauthWorkspaces] = useState<readonly Workspace[]>(
    []
  );
  const [resendAvailableIn, setResendAvailableIn] = useState(0);
  const [otpValue, setOtpValue] = useState("");
  const otpRef = useRef<HTMLInputElement>(null);
  const verifyInFlight = useRef(false);

  // `import.meta.env.SSR` is replaced at build time, so the server render gets
  // an empty query and the browser gets the real one without sniffing globals.
  const params = new URLSearchParams(
    import.meta.env.SSR ? "" : location.search
  );
  const oauthQuery = signedOAuthQuery(params);
  const userCode = params.get("user_code")?.trim() ?? "";
  const agentId = params.get("agent_id")?.trim() ?? "";
  const agentCode = params.get("code")?.trim() ?? "";

  useEffect(() => {
    if (phase !== "code" || resendAvailableIn <= 0) return;
    const timeout = window.setTimeout(() => {
      setResendAvailableIn((seconds) => Math.max(0, seconds - 1));
    }, 1000);
    return () => window.clearTimeout(timeout);
  }, [phase, resendAvailableIn]);

  const ensureWorkspace = useCallback(async (): Promise<WorkspaceBody> => {
    const body = await request<WorkspaceBody>(
      "/onboarding/ensure-personal-organization",
      { method: "POST", body: "{}" }
    );
    setWorkspace(body);
    return body;
  }, []);

  const signedIn = useCallback(async (): Promise<boolean> => {
    // Signed-out responses are the JSON literal `null`, NOT a 401, so the status
    // code alone cannot answer this question.
    const body = await request<SessionBody | null>("/get-session");
    setAccount(body?.user?.email ?? "");
    return Boolean(body?.session);
  }, []);

  const signOut = useCallback(async (): Promise<void> => {
    await request("/sign-out", { method: "POST", body: "{}" });
    location.reload();
  }, []);

  const finishOAuthLogin = useCallback(async (): Promise<boolean> => {
    if (!oauthQuery) return false;
    // `/oauth2/continue` rejects a body without one of `selected`, `created`
    // or `postLogin` ("Missing parameters"). `selected` resumes authorization
    // for the account that just signed in; `created` does the same for one
    // that was just registered.
    const body =
      page === "sign-up"
        ? { oauth_query: oauthQuery, created: true }
        : { oauth_query: oauthQuery, selected: true };
    const result = await request<RedirectBody>("/oauth2/continue", {
      method: "POST",
      body: JSON.stringify(body),
    });
    location.assign(result.url);
    return true;
  }, [oauthQuery, page]);

  const confirmWorkspace = useCallback(() => {
    if (!chosen) {
      setError("Choose a workspace first.");
      return;
    }
    setBusy(true);
    setError("");
    setStatus("Switching workspace…");
    void request("/organization/set-active", {
      method: "POST",
      body: JSON.stringify({ organizationId: chosen }),
    })
      .then(async () => {
        if (page === "consent") {
          setStatus("");
          setBusy(false);
          setPhase("consent");
          return;
        }
        await finishOAuthLogin();
      })
      .catch((cause: unknown) => {
        setStatus("");
        setError(message(cause, "Could not switch workspace"));
        setBusy(false);
      });
  }, [chosen, finishOAuthLogin, page]);

  /**
   * Show the workspace step when the account belongs to more than one
   * workspace. Preselects like the device flow: the only shared workspace,
   * else the one already active. Resolves `false` when there is no choice.
   */
  const askForWorkspace = useCallback(async (): Promise<boolean> => {
    const rows =
      await request<readonly OrganizationRow[]>("/organization/list");
    if (rows.length < 2) return false;
    const session = await request<{
      readonly session?: { readonly activeOrganizationId?: string | null };
    } | null>("/get-session");
    const options = rows.map((row) => ({
      id: row.id,
      name: row.name,
      role: "",
      personal: (row.slug ?? "").startsWith("personal-"),
    }));
    const shared = options.filter((option) => !option.personal);
    setOauthWorkspaces(options);
    setChosen(
      shared.length === 1
        ? shared[0]!.id
        : (session?.session?.activeOrganizationId ?? null)
    );
    setStatus("");
    setPhase("workspace");
    return true;
  }, []);

  /** Everything that happens once a session exists, for each page. */
  const afterSignIn = useCallback(async (): Promise<void> => {
    const { organization, created } = await ensureWorkspace();

    if (page === "device") {
      if (!userCode) {
        setError(
          "This device link is missing its user code. Run sigiro auth login again."
        );
        return;
      }
      setDetails(
        await request<DeviceDetails>(
          `/device?user_code=${encodeURIComponent(userCode)}`
        )
      );
      const options = await request<DeviceWorkspaces>(
        `/device/workspaces?user_code=${encodeURIComponent(userCode)}`
      );
      setGrantable(options);
      setChosen(options.suggested);
      setStatus("");
      setPhase("request");
      return;
    }

    if (page === "agent-device") {
      if (!agentId || !agentCode) {
        setError(
          "This agent approval link is incomplete. Start the connection again."
        );
        return;
      }
      setDetails({
        client_id: agentId,
        scope: "Capabilities requested by this agent",
        resource: location.origin,
      });
      setStatus(
        "Confirm this code matches the requesting agent before approving."
      );
      setPhase("request");
      return;
    }

    // A browser OAuth client gets a token for the session's active workspace.
    // Sign-in always activates the personal one, so a member of several must
    // be asked, or a shared workspace's dashboard silently opens empty.
    if (oauthQuery && (await askForWorkspace())) return;

    if (page === "consent") {
      setPhase("consent");
      return;
    }

    if (await finishOAuthLogin()) return;
    setSuccess(
      created
        ? `Your Sigiro account and workspace “${organization.name}” are ready.`
        : `Signed in to ${organization.name}.`
    );
    setStatus("");
    setPhase("done");
  }, [
    agentCode,
    agentId,
    askForWorkspace,
    ensureWorkspace,
    finishOAuthLogin,
    oauthQuery,
    page,
    userCode,
  ]);

  useEffect(() => {
    void (async () => {
      try {
        if (await signedIn()) {
          await afterSignIn();
          return;
        }
        if (page === "consent") {
          location.assign(`/sign-in?${oauthQuery ?? ""}`);
          return;
        }
        setPhase("email");
      } catch (cause) {
        setError(message(cause, "Authentication failed"));
        setPhase("email");
      }
    })();
  }, [afterSignIn, oauthQuery, page, signedIn]);

  const deliverCode = useCallback((address: string, resend: boolean): void => {
    setBusy(true);
    setError("");
    setStatus(resend ? "Sending a new code…" : "Sending your code…");
    void request("/email-otp/send-verification-otp", {
      method: "POST",
      body: JSON.stringify({ email: address, type: "sign-in" }),
    })
      .then(() => {
        setEmail(address);
        setPhase("code");
        setResendAvailableIn(resendCooldownSeconds);
        if (resend) {
          setStatus("A new verification code was sent.");
          setOtpValue("");
        } else {
          setOtpValue("");
          setStatus("");
        }
      })
      .catch((cause: unknown) => {
        setStatus("");
        setError(message(cause, "Could not send the verification code"));
      })
      .finally(() => {
        setBusy(false);
      });
  }, []);

  const sendCode = useCallback(
    (event: React.FormEvent) => {
      event.preventDefault();
      deliverCode(email.trim().toLowerCase(), false);
    },
    [deliverCode, email]
  );

  /** Verify automatically at six digits or explicitly through the form submit button. */
  const verify = useCallback(
    (code: string) => {
      if (verifyInFlight.current) return;
      verifyInFlight.current = true;
      setBusy(true);
      setError("");
      setStatus("Verifying…");
      // The name is optional and only used if this code creates the account:
      // better-auth ignores it for an address that already has one.
      const trimmedName = name.trim();
      const body = trimmedName
        ? { email, otp: code, name: trimmedName }
        : { email, otp: code };
      void request("/sign-in/email-otp", {
        method: "POST",
        body: JSON.stringify(body),
      })
        .then(() => {
          setAccount(email);
          return afterSignIn();
        })
        .catch((cause: unknown) => {
          setStatus("");
          setError(verificationMessage(cause));
          window.requestAnimationFrame(() => otpRef.current?.select());
        })
        .finally(() => {
          verifyInFlight.current = false;
          setBusy(false);
        });
    },
    [afterSignIn, email, name, page]
  );

  const decide = useCallback(
    (
      approve: boolean,
      path: string,
      body: DecisionBody,
      approved: string,
      denied: string
    ) => {
      setBusy(true);
      setError("");
      setStatus(approve ? "Approving…" : "Denying…");
      void request(path, { method: "POST", body: JSON.stringify(body) })
        .then(() => {
          setSuccess(approve ? approved : denied);
          setStatus("");
          setPhase("done");
        })
        .catch((cause: unknown) => {
          setStatus("");
          setError(message(cause, "Could not process the request"));
        })
        .finally(() => {
          setBusy(false);
        });
    },
    []
  );

  const decideRequest = useCallback(
    (approve: boolean) => {
      if (page === "agent-device") {
        decide(
          approve,
          "/agent/approve-capability",
          {
            agent_id: agentId,
            user_code: agentCode,
            action: approve ? "approve" : "deny",
          },
          "Return to your agent. Access is approved.",
          "The agent request was denied."
        );
        return;
      }
      const target = grantable?.workspaces.find(
        (option) => option.id === chosen
      );
      if (approve && !target) {
        setError("Choose the workspace to connect first.");
        return;
      }
      decide(
        approve,
        approve ? "/device/approve" : "/device/deny",
        approve ? { userCode, organizationId: target!.id } : { userCode },
        `Return to your terminal. The CLI is connected to ${target?.name ?? "your workspace"}.`,
        "The device request was denied."
      );
    },
    [agentCode, agentId, chosen, decide, grantable, page, userCode]
  );

  const decideConsent = useCallback(
    (accept: boolean) => {
      if (!oauthQuery) {
        setError("This authorization request is missing its signed query.");
        return;
      }
      setBusy(true);
      setStatus(accept ? "Approving…" : "Denying…");
      void request<RedirectBody>("/oauth2/consent", {
        method: "POST",
        body: JSON.stringify({ accept, oauth_query: oauthQuery }),
      })
        .then((result) => {
          location.assign(result.url);
        })
        .catch((cause: unknown) => {
          setStatus("");
          setError(message(cause, "Could not process consent"));
          setBusy(false);
        });
    },
    [oauthQuery]
  );

  /**
   * The live region stays in the DOM so assistive tech keeps watching it, but it
   * only occupies space once it has something to say — reserving a line
   * unconditionally left a visible gap under every form at rest.
   */
  /**
   * The heading tracks the phase. `title`/`intro` are the server-rendered pair
   * for the first step, so SSR emits the correct `<h1>` and hydration does not
   * swap it; only a later step overrides the intro.
   */
  const heading =
    phase === "code"
      ? { title, intro: `Enter the six-digit code we sent to ${email}.` }
      : phase === "workspace"
        ? {
            title,
            intro: "Choose the workspace this app will read.",
          }
        : phase === "request"
          ? {
              title,
              intro: "Confirm this request came from you before approving it.",
            }
          : { title, intro };

  const header = (
    <div className="mb-6">
      <h1
        id="title"
        className="text-foreground text-[26px] leading-[1.15] font-semibold tracking-[-0.035em]"
      >
        {heading.title}
      </h1>
      <p className="text-muted-foreground mt-3 text-[15px] leading-relaxed">
        {heading.intro}
      </p>
    </div>
  );

  const statusLine = (
    <>
      <p
        id="status"
        role="status"
        aria-live="polite"
        className={`text-muted-foreground text-[13px] ${status ? "" : "sr-only"}`}
      >
        {status}
      </p>
      {error && phase !== "code" && phase !== "email" && (
        <p
          id="error"
          role="alert"
          className="text-[13px] text-red-600 dark:text-red-400"
        >
          {error}
        </p>
      )}
    </>
  );

  if (phase === "loading") {
    return (
      <>
        {header}
        <div className="h-24" aria-busy="true" />
      </>
    );
  }

  if (phase === "done") {
    const accountSetup = page === "sign-in" || page === "sign-up";
    return (
      <>
        {header}
        <div id="success" className="grid gap-3">
          <h2 className="text-foreground text-[20px] font-semibold tracking-[-0.02em]">
            You&rsquo;re ready.
          </h2>
          <p
            id="success-message"
            className="text-muted-foreground text-[15px] leading-relaxed"
          >
            {success}
          </p>
          {accountSetup && (
            <div className="mt-2 grid gap-3">
              <p className="text-muted-foreground text-[14px] leading-relaxed">
                {workspace?.created
                  ? "Your workspace is empty until something sends it telemetry. Next: install the CLI, connect it to this same workspace, and point an OpenTelemetry exporter at it."
                  : "Install the CLI, then connect it to the same workspace."}
              </p>
              {workspace?.created && (
                <p
                  id="join-hint"
                  className="text-muted-foreground text-[13px] leading-relaxed"
                >
                  Joining a team that already uses Sigiro? Ask one of its owners
                  to add {account || "your email"}. You choose the workspace
                  when you connect the CLI.
                </p>
              )}
              {(
                [
                  ["install command", INSTALL_COMMAND],
                  ["login command", LOGIN_COMMAND],
                ] as const
              ).map(([label, command]) => (
                <figure
                  key={label}
                  className="bg-card min-w-0 rounded-lg border p-[4px] shadow-sm"
                >
                  <figcaption className="flex h-8 items-center gap-2 pr-1 pl-2.5">
                    <span className="text-muted-foreground min-w-0 flex-1 truncate font-mono text-xs tabular-nums">
                      {label}
                    </span>
                    <CopySource code={command} label={label} />
                  </figcaption>
                  <code className="bg-muted text-muted-foreground block rounded-md px-3 py-2.5 font-mono text-[12px] leading-relaxed break-normal whitespace-pre-wrap">
                    {command}
                  </code>
                </figure>
              ))}
              <a
                href="/docs/how-to/hosted-onboarding#1-connect-the-cli"
                className={`${secondary} justify-self-start`}
              >
                Read the CLI guide
              </a>
            </div>
          )}
        </div>
      </>
    );
  }

  if (phase === "workspace") {
    return (
      <>
        {header}
        <div id="workspace-flow" className="grid gap-4">
          <fieldset className="grid gap-1.5">
            <legend className="text-muted-foreground text-[11px] tracking-[0.08em] uppercase">
              Workspace
            </legend>
            {oauthWorkspaces.map((option) => (
              <label
                key={option.id}
                className="text-foreground hover:bg-muted flex cursor-pointer items-center gap-2.5 rounded-md px-1 py-1 text-[14px]"
              >
                <input
                  type="radio"
                  name="workspace"
                  value={option.id}
                  checked={chosen === option.id}
                  onChange={() => {
                    setChosen(option.id);
                    setError("");
                  }}
                />
                <span className="font-medium">{option.name}</span>
                {option.personal && (
                  <span className="text-muted-foreground text-[12px]">
                    personal
                  </span>
                )}
              </label>
            ))}
          </fieldset>
          <div className="flex gap-2.5">
            <button
              id="confirm-workspace"
              type="button"
              disabled={busy || !chosen}
              onClick={confirmWorkspace}
              className={primary}
            >
              Continue
            </button>
          </div>
          {statusLine}
        </div>
      </>
    );
  }

  if (phase === "consent") {
    return (
      <>
        {header}
        <div id="consent-flow" className="grid gap-4">
          <p className="text-muted-foreground text-[15px] leading-relaxed">
            The requesting client will receive only the scopes in its signed
            authorization request.
          </p>
          <div className="flex gap-2.5">
            <button
              id="accept-consent"
              type="button"
              disabled={busy}
              onClick={() => decideConsent(true)}
              className={primary}
            >
              Approve access
            </button>
            <button
              id="deny-consent"
              type="button"
              disabled={busy}
              onClick={() => decideConsent(false)}
              className={secondary}
            >
              Deny
            </button>
          </div>
          {statusLine}
        </div>
      </>
    );
  }

  if (phase === "request") {
    const isDevice = page === "device";
    const workspaces = grantable?.workspaces ?? [];
    const target = workspaces.find((option) => option.id === chosen);
    const expires = grantable?.expires_at
      ? new Date(grantable.expires_at).toLocaleTimeString([], {
          hour: "2-digit",
          minute: "2-digit",
        })
      : null;
    const rows: ReadonlyArray<readonly [string, string, string]> = isDevice
      ? [
          [
            "client-id",
            "Client",
            grantable?.client_name ?? details?.client_id ?? "Unknown client",
          ],
          ["scope", "Access", describeScopes(details?.scope)],
          ...(expires
            ? [["expires", "Code expires", `at ${expires}`] as const]
            : []),
        ]
      : [
          ["client-id", "Agent", details?.client_id ?? "Unknown agent"],
          ["scope", "Access", details?.scope ?? "No scopes requested"],
        ];
    return (
      <>
        {header}
        <div id="device-flow" className="grid gap-4">
          {account && (
            <p id="signed-in-as" className="text-muted-foreground text-[13px]">
              Signed in as{" "}
              <span className="text-foreground font-medium">{account}</span>.{" "}
              <button
                id="sign-out"
                type="button"
                onClick={() => void signOut()}
                className="hover:text-foreground font-medium underline underline-offset-4"
              >
                Not you? Sign out
              </button>
            </p>
          )}
          <div>
            <p className="text-muted-foreground text-[13px] font-medium">
              Confirm this code matches your terminal
            </p>
            <output
              id="user-code"
              className="text-foreground mt-2 block font-mono text-[30px] leading-none font-bold tracking-[0.16em]"
            >
              {page === "agent-device" ? agentCode : userCode}
            </output>
          </div>
          {isDevice && (
            <fieldset
              id="workspace"
              className="border-border grid gap-2 border-t pt-2.5"
            >
              <legend className="text-muted-foreground text-[11px] tracking-[0.08em] uppercase">
                Workspace this CLI will use
              </legend>
              {workspaces.length === 0 && (
                <p className="text-[13px] text-red-600 dark:text-red-400">
                  Your account has no workspace this client can use. Ask a
                  workspace owner to add {account || "you"}.
                </p>
              )}
              {workspaces.length === 1 && (
                <p
                  id="workspace-name"
                  className="text-foreground text-[15px] font-semibold"
                >
                  {workspaces[0]!.name}
                </p>
              )}
              {workspaces.length > 1 &&
                workspaces.map((option) => (
                  <label
                    key={option.id}
                    className="text-foreground hover:bg-muted flex cursor-pointer items-center gap-2.5 rounded-md px-1 py-1 text-[14px]"
                  >
                    <input
                      type="radio"
                      name="workspace"
                      value={option.id}
                      checked={chosen === option.id}
                      onChange={() => {
                        setChosen(option.id);
                        setError("");
                      }}
                    />
                    <span className="font-medium">{option.name}</span>
                    <span className="text-muted-foreground text-[12px]">
                      {option.personal ? "personal" : option.role}
                    </span>
                  </label>
                ))}
              {workspaces.length > 1 && (
                <p className="text-muted-foreground text-[12px]">
                  Everything the CLI sends and reads goes to this workspace
                  only. Run sigiro auth login again to switch.
                </p>
              )}
            </fieldset>
          )}
          <dl id="request-details" className="grid gap-2.5">
            {rows.map(([id, term, value]) => (
              <div key={id} className="border-border border-t pt-2.5">
                <dt className="text-muted-foreground text-[11px] tracking-[0.08em] uppercase">
                  {term}
                </dt>
                <dd
                  id={id}
                  className="text-muted-foreground mt-1 text-[13px] break-words"
                >
                  {value}
                </dd>
              </div>
            ))}
          </dl>
          <div className="flex flex-wrap gap-2.5">
            <button
              id="approve"
              type="button"
              disabled={busy || (isDevice && !target)}
              onClick={() => decideRequest(true)}
              className={primary}
            >
              {isDevice
                ? target
                  ? `Connect to ${target.name}`
                  : "Choose a workspace"
                : "Approve request"}
            </button>
            <button
              id="deny"
              type="button"
              disabled={busy}
              onClick={() => decideRequest(false)}
              className={secondary}
            >
              Deny
            </button>
          </div>
          {statusLine}
        </div>
      </>
    );
  }

  if (phase === "code") {
    return (
      <>
        {header}
        <form
          id="otp-form"
          className="grid gap-4"
          aria-busy={busy}
          onSubmit={(event) => {
            event.preventDefault();
            if (otpValue.length === 6) verify(otpValue);
          }}
        >
          <div className="grid justify-items-center gap-2">
            <InputOTP
              ref={otpRef}
              name="verification-code"
              maxLength={6}
              inputMode="numeric"
              pattern="^[0-9]*$"
              autoComplete="one-time-code"
              autoFocus
              aria-label="Verification code"
              aria-invalid={error ? true : undefined}
              value={otpValue}
              onChange={(value) => {
                setOtpValue(value);
                if (error) setError("");
              }}
              onComplete={verify}
            >
              <InputOTPGroup>
                {[0, 1, 2, 3, 4, 5].map((index) => (
                  <InputOTPSlot key={index} index={index} />
                ))}
              </InputOTPGroup>
            </InputOTP>
            <p
              role={error ? "alert" : undefined}
              className={
                error
                  ? "text-destructive text-sm"
                  : "text-muted-foreground text-sm"
              }
            >
              {error || "Enter or paste the six-digit code."}
            </p>
          </div>
          <button
            type="submit"
            disabled={busy || otpValue.length !== 6}
            className={`${primary} w-full`}
          >
            {busy ? "Verifying…" : "Verify code"}
          </button>
          <div className="flex flex-wrap items-center gap-1">
            <button
              id="resend"
              type="button"
              disabled={busy || resendAvailableIn > 0}
              onClick={() => deliverCode(email, true)}
              className="text-muted-foreground hover:text-foreground disabled:text-muted-foreground dark:disabled:text-muted-foreground inline-flex h-10 items-center rounded-md px-2 text-[13px] font-medium underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#4568FF] disabled:cursor-not-allowed"
            >
              {resendAvailableIn > 0
                ? `Resend code in ${resendAvailableIn}s`
                : "Resend code"}
            </button>
            <button
              id="restart"
              type="button"
              disabled={busy}
              onClick={() => {
                setPhase("email");
                setStatus("");
                setError("");
                setResendAvailableIn(0);
                setOtpValue("");
              }}
              className="text-muted-foreground hover:text-foreground inline-flex h-10 items-center rounded-md px-2 text-[13px] font-medium underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#4568FF] disabled:opacity-60"
            >
              Use another email
            </button>
          </div>
          {statusLine}
        </form>
      </>
    );
  }

  return (
    <>
      {header}
      {/*
        `gap-1`, not `gap-3`: each field already reserves room above it for the
        raised label and below it for a hint, so a 12px gap on top of that left
        ~54px of air between Name and Email.
      */}
      <form id="email-form" className="grid gap-1" onSubmit={sendCode}>
        {/*
          One card for both sign-in and sign-up: a one-time code creates the
          account the first time an address uses it, so there is nothing a
          separate sign-up page could ask that this one cannot.
        */}
        {(page === "sign-in" || page === "sign-up") && (
          <div className="grid gap-2">
            <Label htmlFor="name">Name (optional)</Label>
            <Input
              id="name"
              name="name"
              autoComplete="name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              disabled={busy}
            />
          </div>
        )}
        <div className="grid gap-2">
          <Label htmlFor="email">Email</Label>
          <Input
            id="email"
            name="email"
            type="email"
            autoComplete="email"
            required
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? "email-error" : undefined}
            disabled={busy}
          />
          {error && (
            <p
              id="email-error"
              role="alert"
              className="text-destructive text-sm"
            >
              {error}
            </p>
          )}
        </div>
        <button type="submit" disabled={busy} className={`${primary} mt-2`}>
          Continue with email
        </button>
        {statusLine}
      </form>
    </>
  );
}
