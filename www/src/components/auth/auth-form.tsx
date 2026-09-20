import { useCallback, useEffect, useRef, useState } from 'react';

import { FloatingLabelInput } from '@/components/interior/floating-label';
import { OtpInput, type OtpInputHandle } from '@/components/interior/otp-input';

/**
 * The entire auth card: email, one-time code, device approval, consent, done.
 *
 * It is one component on purpose. The first port of these pages kept a vanilla
 * `auth-ui.ts` module driving hidden `<div>`s and bolted a React field on top,
 * which left two owners for one flow — the island holding the email and code,
 * the module deciding what happened after. Phase is state; this owns it.
 *
 * Built from the interior.dev components the site already vendors under
 * `src/components/interior/`. DESIGN.md §3b names the OTP cell as the reference
 * every other field copies, so the screen where that field actually appears
 * should be the component itself rather than an imitation in Tailwind classes.
 * That brings the per-cell error and success branches, paste-into-any-cell,
 * arrow-key navigation and the §5 spring motion for free.
 *
 * IDs are load-bearing: `check-signup-browser.ts` and `check-device-flow.ts`
 * drive production by selector, so `#email-form`, `#otp-form`, `#approve` and
 * `input[name="email"]` survive the rewrite. The code is six cells now rather
 * than one input — filling the first cell with all six digits is a paste, which
 * the component spreads across the row, so the checks still do what a user does.
 */

export type AuthPage = 'sign-in' | 'sign-up' | 'device' | 'agent-device' | 'consent';

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

type Phase = 'loading' | 'email' | 'code' | 'request' | 'consent' | 'done';

const api = '/api/auth';

interface ErrorBody {
  readonly message?: string;
  readonly error_description?: string;
  readonly error?: string;
}

interface SessionBody {
  readonly session?: unknown;
}

interface RedirectBody {
  readonly redirect_uri: string;
}

/** The body of a device or agent approval decision. */
type DecisionBody =
  | { readonly userCode: string }
  | { readonly agent_id: string; readonly user_code: string; readonly action: 'approve' | 'deny' };

interface DeviceDetails {
  readonly client_id?: string;
  readonly scope?: string;
  readonly resource?: string | readonly string[];
}

const request = async <T,>(path: string, options: RequestInit = {}): Promise<T> => {
  const response = await fetch(api + path, {
    credentials: 'same-origin',
    ...options,
    headers:
      options.body === undefined
        ? options.headers
        : { 'content-type': 'application/json', ...options.headers },
  });
  // SAFETY: every auth endpoint answers JSON; an unparseable body becomes an
  // empty object and surfaces as the generic failure below.
  const body = (await response.json().catch(() => ({}))) as T & ErrorBody;
  if (!response.ok) {
    throw new Error(body.message ?? body.error_description ?? body.error ?? 'Request failed');
  }
  return body;
};

const message = (cause: unknown, fallback: string): string =>
  cause instanceof Error ? cause.message : fallback;

const primary =
  'inline-flex h-10 items-center justify-center rounded-[10px] bg-stone-900 px-4 text-[14px] ' +
  'font-semibold text-white transition-colors duration-150 hover:bg-stone-800 ' +
  'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#4568FF] ' +
  'disabled:opacity-60 dark:bg-stone-100 dark:text-stone-900 dark:hover:bg-white';

const secondary =
  'inline-flex h-10 items-center justify-center rounded-[10px] border-2 border-stone-200 px-4 ' +
  'text-[14px] font-semibold text-stone-700 transition-colors duration-150 hover:bg-stone-100 ' +
  'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#4568FF] ' +
  'disabled:opacity-60 dark:border-white/[0.08] dark:text-stone-300 dark:hover:bg-white/[0.04]';

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
    page === 'sign-in' || page === 'sign-up' ? 'email' : 'loading',
  );
  const [email, setEmail] = useState('');
  const [name, setName] = useState('');
  const [status, setStatus] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [success, setSuccess] = useState('');
  const [details, setDetails] = useState<DeviceDetails | null>(null);
  const otpRef = useRef<OtpInputHandle>(null);

  // `import.meta.env.SSR` is replaced at build time, so the server render gets
  // an empty query and the browser gets the real one without sniffing globals.
  const params = new URLSearchParams(import.meta.env.SSR ? '' : location.search);
  const oauthQuery = params.get('oauth_query');
  const userCode = params.get('user_code')?.trim() ?? '';
  const agentId = params.get('agent_id')?.trim() ?? '';
  const agentCode = params.get('code')?.trim() ?? '';

  const ensureWorkspace = useCallback(async (): Promise<void> => {
    await request('/onboarding/ensure-personal-organization', { method: 'POST', body: '{}' });
  }, []);

  const signedIn = useCallback(async (): Promise<boolean> => {
    // Signed-out responses are the JSON literal `null`, NOT a 401, so the status
    // code alone cannot answer this question.
    const body = await request<SessionBody | null>('/get-session');
    return Boolean(body?.session);
  }, []);

  const finishOAuthLogin = useCallback(async (): Promise<boolean> => {
    if (!oauthQuery) return false;
    const body =
      page === 'sign-up' ? { oauth_query: oauthQuery, created: true } : { oauth_query: oauthQuery };
    const result = await request<RedirectBody>('/oauth2/continue', {
      method: 'POST',
      body: JSON.stringify(body),
    });
    location.assign(result.redirect_uri);
    return true;
  }, [oauthQuery, page]);

  /** Everything that happens once a session exists, for each page. */
  const afterSignIn = useCallback(async (): Promise<void> => {
    await ensureWorkspace();

    if (page === 'device') {
      if (!userCode) {
        setError('This device link is missing its user code. Run sigiro auth login again.');
        return;
      }
      setDetails(await request<DeviceDetails>(`/device?user_code=${encodeURIComponent(userCode)}`));
      setStatus('Review the request before approving it.');
      setPhase('request');
      return;
    }

    if (page === 'agent-device') {
      if (!agentId || !agentCode) {
        setError('This agent approval link is incomplete. Start the connection again.');
        return;
      }
      setDetails({
        client_id: agentId,
        scope: 'Capabilities requested by this agent',
        resource: location.origin,
      });
      setStatus('Confirm this code matches the requesting agent before approving.');
      setPhase('request');
      return;
    }

    if (page === 'consent') {
      setPhase('consent');
      return;
    }

    if (await finishOAuthLogin()) return;
    setSuccess('Your Sigiro account and personal workspace are active.');
    setStatus('');
    setPhase('done');
  }, [agentCode, agentId, ensureWorkspace, finishOAuthLogin, page, userCode]);

  useEffect(() => {
    void (async () => {
      try {
        if (await signedIn()) {
          await afterSignIn();
          return;
        }
        if (page === 'consent') {
          location.assign(`/sign-in?oauth_query=${encodeURIComponent(oauthQuery ?? '')}`);
          return;
        }
        setPhase('email');
      } catch (cause) {
        setError(message(cause, 'Authentication failed'));
        setPhase('email');
      }
    })();
  }, [afterSignIn, oauthQuery, page, signedIn]);

  const sendCode = useCallback(
    (event: React.FormEvent) => {
      event.preventDefault();
      const address = email.trim().toLowerCase();
      setBusy(true);
      setError('');
      setStatus('Sending your code…');
      void request('/email-otp/send-verification-otp', {
        method: 'POST',
        body: JSON.stringify({ email: address, type: 'sign-in' }),
      })
        .then(() => {
          setEmail(address);
          setPhase('code');
          // No status here: the OTP field's own hint already says where the
          // code went, and two lines saying the same thing is noise.
          setStatus('');
        })
        .catch((cause: unknown) => {
          setStatus('');
          setError(message(cause, 'Could not send the code'));
        })
        .finally(() => {
          setBusy(false);
        });
    },
    [email],
  );

  /**
   * Verification fires when the sixth cell fills, not on a button press. The
   * code is fixed-length, so a second gesture after the last digit carries no
   * information the form does not already have.
   */
  const verify = useCallback(
    (code: string) => {
      setBusy(true);
      setError('');
      setStatus('Verifying…');
      const body =
        page === 'sign-up' ? { email, otp: code, name: name.trim() } : { email, otp: code };
      void request('/sign-in/email-otp', { method: 'POST', body: JSON.stringify(body) })
        .then(afterSignIn)
        .catch((cause: unknown) => {
          setStatus('');
          setError(message(cause, 'Verification failed'));
          otpRef.current?.clear();
        })
        .finally(() => {
          setBusy(false);
        });
    },
    [afterSignIn, email, name, page],
  );

  const decide = useCallback(
    (approve: boolean, path: string, body: DecisionBody, approved: string, denied: string) => {
      setBusy(true);
      setError('');
      setStatus(approve ? 'Approving…' : 'Denying…');
      void request(path, { method: 'POST', body: JSON.stringify(body) })
        .then(() => {
          setSuccess(approve ? approved : denied);
          setStatus('');
          setPhase('done');
        })
        .catch((cause: unknown) => {
          setStatus('');
          setError(message(cause, 'Could not process the request'));
        })
        .finally(() => {
          setBusy(false);
        });
    },
    [],
  );

  const decideRequest = useCallback(
    (approve: boolean) => {
      if (page === 'agent-device') {
        decide(
          approve,
          '/agent/approve-capability',
          { agent_id: agentId, user_code: agentCode, action: approve ? 'approve' : 'deny' },
          'Return to your agent. Access is approved.',
          'The agent request was denied.',
        );
        return;
      }
      decide(
        approve,
        approve ? '/device/approve' : '/device/deny',
        { userCode },
        'Return to your terminal. The CLI is connected.',
        'The device request was denied.',
      );
    },
    [agentCode, agentId, decide, page, userCode],
  );

  const decideConsent = useCallback(
    (accept: boolean) => {
      if (!oauthQuery) {
        setError('This authorization request is missing its signed query.');
        return;
      }
      setBusy(true);
      setStatus(accept ? 'Approving…' : 'Denying…');
      void request<RedirectBody>('/oauth2/consent', {
        method: 'POST',
        body: JSON.stringify({ accept, oauth_query: oauthQuery }),
      })
        .then((result) => {
          location.assign(result.redirect_uri);
        })
        .catch((cause: unknown) => {
          setStatus('');
          setError(message(cause, 'Could not process consent'));
          setBusy(false);
        });
    },
    [oauthQuery],
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
    phase === 'code'
      ? { title, intro: `Enter the six-digit code we sent to ${email}.` }
      : phase === 'request'
        ? { title, intro: 'Confirm this request came from you before approving it.' }
        : { title, intro };

  const header = (
    <div className="mb-6">
      <p className="font-mono text-[11px] font-semibold uppercase tracking-[0.12em] text-[#4568FF] dark:text-[#93B0FF]">
        hosted sigiro
      </p>
      <h1
        id="title"
        className="mt-2.5 text-[26px] font-semibold leading-[1.15] tracking-[-0.035em] text-stone-900 dark:text-stone-50"
      >
        {heading.title}
      </h1>
      <p className="mt-3 text-[15px] leading-relaxed text-stone-600 dark:text-stone-400">
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
        className={`text-[13px] text-stone-500 dark:text-stone-400 ${status ? '' : 'sr-only'}`}
      >
        {status}
      </p>
      {error && phase !== 'code' && phase !== 'email' && (
        <p id="error" role="alert" className="text-[13px] text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
    </>
  );

  if (phase === 'loading') {
    return (
      <>
        {header}
        <div className="h-24" aria-busy="true" />
      </>
    );
  }

  if (phase === 'done') {
    return (
      <>
        {header}
        <div id="success" className="grid gap-3">
          <h2 className="text-[20px] font-semibold tracking-[-0.02em] text-stone-900 dark:text-stone-50">
            You&rsquo;re ready.
          </h2>
          <p
            id="success-message"
            className="text-[15px] leading-relaxed text-stone-600 dark:text-stone-400"
          >
            {success}
          </p>
          <a href="/docs/how-to/hosted-onboarding" className={`${primary} mt-2 justify-self-start`}>
            Continue to hosted setup
          </a>
        </div>
      </>
    );
  }

  if (phase === 'consent') {
    return (
      <>
        {header}
        <div id="consent-flow" className="grid gap-4">
          <p className="text-[15px] leading-relaxed text-stone-600 dark:text-stone-400">
            The requesting client will receive only the scopes in its signed authorization request.
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

  if (phase === 'request') {
    const resource = Array.isArray(details?.resource)
      ? details.resource.join(', ')
      : (details?.resource ?? 'No resource requested');
    return (
      <>
        {header}
        <div id="device-flow" className="grid gap-4">
          <div>
            <p className="text-[13px] font-medium text-stone-600 dark:text-stone-400">
              Confirm this code matches your terminal
            </p>
            <output
              id="user-code"
              className="mt-2 block font-mono text-[30px] font-bold leading-none tracking-[0.16em] text-stone-900 dark:text-stone-50"
            >
              {page === 'agent-device' ? agentCode : userCode}
            </output>
          </div>
          <dl id="request-details" className="grid gap-2.5">
            {(
              [
                ['client-id', 'Client', details?.client_id ?? 'Unknown client'],
                ['scope', 'Access', details?.scope ?? 'No scopes requested'],
                ['resource', 'Resource', resource],
              ] as const
            ).map(([id, term, value]) => (
              <div key={id} className="border-t border-stone-200 pt-2.5 dark:border-white/[0.08]">
                <dt className="text-[11px] uppercase tracking-[0.08em] text-stone-500">{term}</dt>
                <dd
                  id={id}
                  className="mt-1 break-words text-[13px] text-stone-700 dark:text-stone-300"
                >
                  {value}
                </dd>
              </div>
            ))}
          </dl>
          <div className="flex gap-2.5">
            <button
              id="approve"
              type="button"
              disabled={busy}
              onClick={() => decideRequest(true)}
              className={primary}
            >
              Approve request
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

  if (phase === 'code') {
    return (
      <>
        {header}
        <form
          id="otp-form"
          className="grid gap-4"
          onSubmit={(event) => {
            event.preventDefault();
          }}
        >
          <OtpInput
            length={6}
            autoFocus
            disabled={busy}
            status={error ? 'error' : 'idle'}
            errorMessage={error}
            hint="Paste the whole code into any cell."
            onChange={() => {
              if (error) setError('');
            }}
            onComplete={verify}
            ref={otpRef}
          />
          <button
            id="restart"
            type="button"
            onClick={() => {
              setPhase('email');
              setStatus('');
              setError('');
            }}
            className="mt-2 justify-self-start text-[13px] font-medium text-stone-500 underline-offset-4 hover:text-stone-800 hover:underline dark:text-stone-400 dark:hover:text-stone-100"
          >
            Use another email
          </button>
          {statusLine}
        </form>
      </>
    );
  }

  return (
    <>
      {header}
      <form id="email-form" className="grid gap-3" onSubmit={sendCode}>
        {/*
          No `maxLength` on the name field: the component renders a live
          `0 / 100` counter when given one, and a character budget on an
          optional display name is noise on the first screen of a sign-up.
        */}
        {page === 'sign-up' && (
          <FloatingLabelInput
            id="name"
            name="name"
            label="Name"
            autoComplete="name"
            value={name}
            onChange={setName}
            disabled={busy}
          />
        )}
        <FloatingLabelInput
          id="email"
          name="email"
          type="email"
          label="Email"
          autoComplete="email"
          required
          value={email}
          onChange={setEmail}
          invalid={Boolean(error)}
          hint={error}
          disabled={busy}
        />
        <button type="submit" disabled={busy} className={`${primary} mt-1`}>
          Send verification code
        </button>
        {statusLine}
      </form>
    </>
  );
}
