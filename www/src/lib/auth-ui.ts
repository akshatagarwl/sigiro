/**
 * Client behaviour for the auth pages.
 *
 * Ported from the `String.raw` blob in `auth/src/ui.ts`, which the auth Worker
 * served at `/auth-ui.js`. That file was untypechecked, unlintable and
 * unbundled: a typo in it was a runtime error in production with no build step
 * that could have caught it. This is the same flow as a real module — type
 * checked, linted and bundled by the site build.
 *
 * It talks to `/api/auth/*` on the same origin, which the Website Worker routes
 * to the auth Worker over the `AUTH` service binding. Same-origin means the
 * session cookie rides along with `credentials: 'same-origin'` and there is no
 * CORS surface at all.
 */

const API = '/api/auth';

type AuthPage = 'sign-in' | 'sign-up' | 'device' | 'agent-device' | 'consent';

const isAuthPage = (value: string | undefined): value is AuthPage =>
  value === 'sign-in' ||
  value === 'sign-up' ||
  value === 'device' ||
  value === 'agent-device' ||
  value === 'consent';

/** Every element the flow touches, resolved once and asserted present. */
const el = <T extends HTMLElement>(selector: string): T => {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`auth-ui: missing required element ${selector}`);
  return found;
};

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

interface OrganizationBody {
  readonly organization?: { readonly id?: string };
}

interface DeviceDetails {
  readonly client_id?: string;
  readonly scope?: string;
  readonly resource?: string | readonly string[];
}

const request = async <T>(path: string, options: RequestInit = {}): Promise<T> => {
  const response = await fetch(API + path, {
    credentials: 'same-origin',
    ...options,
    headers:
      options.body === undefined
        ? options.headers
        : { 'content-type': 'application/json', ...options.headers },
  });
  // SAFETY: every auth endpoint answers JSON; a body that does not parse is
  // treated as an empty object and surfaces as the generic failure below.
  const body = (await response.json().catch(() => ({}))) as T & ErrorBody;
  if (!response.ok) {
    throw new Error(body.message ?? body.error_description ?? body.error ?? 'Request failed');
  }
  return body;
};

const start = (): void => {
  const page = document.querySelector<HTMLElement>('[data-page]')?.dataset.page;
  if (!isAuthPage(page)) throw new Error(`auth-ui: unknown page ${page ?? '(none)'}`);

  const status = el('#status');
  const error = el('#error');
  const authFlow = el('#auth-flow');
  const emailForm = el<HTMLFormElement>('#email-form');
  const otpForm = el<HTMLFormElement>('#otp-form');
  const nameInput = el<HTMLInputElement>('#name');
  const emailInput = el<HTMLInputElement>('#email');
  const otpInput = el<HTMLInputElement>('#otp');
  let email = '';

  const setStatus = (message: string): void => {
    status.textContent = message;
    error.hidden = true;
  };
  const setError = (message: string): void => {
    error.textContent = message;
    error.hidden = false;
    status.textContent = '';
  };

  // The caller only needs this to have succeeded — a failure throws — so it
  // returns nothing rather than leaking an unparsed body.
  const ensureWorkspace = async (): Promise<void> => {
    await request<OrganizationBody>('/onboarding/ensure-personal-organization', {
      method: 'POST',
      body: '{}',
    });
  };

  const signedIn = async (): Promise<boolean> => {
    // Signed-out responses are the JSON literal `null`, NOT a 401, so the
    // status code alone cannot answer this question.
    const body = await request<SessionBody | null>('/get-session');
    return Boolean(body?.session);
  };

  const params = new URLSearchParams(location.search);
  const oauthQuery = params.get('oauth_query');
  const userCode = params.get('user_code')?.trim() ?? '';
  const agentId = params.get('agent_id')?.trim() ?? '';
  const agentCode = params.get('code')?.trim() ?? '';

  const finishOAuthLogin = async (): Promise<boolean> => {
    if (!oauthQuery) return false;
    const body =
      page === 'sign-up' ? { oauth_query: oauthQuery, created: true } : { oauth_query: oauthQuery };
    const result = await request<RedirectBody>('/oauth2/continue', {
      method: 'POST',
      body: JSON.stringify(body),
    });
    location.assign(result.redirect_uri);
    return true;
  };

  const showSuccess = (message: string): void => {
    authFlow.hidden = true;
    el('#device-flow').hidden = true;
    el('#consent-flow').hidden = true;
    el('#success-message').textContent = message;
    el('#success').hidden = false;
    setStatus('');
  };

  const showAuth = (): void => {
    authFlow.hidden = false;
    if (page === 'sign-up') {
      nameInput.required = true;
      return;
    }
    el('#name-label').hidden = true;
    nameInput.hidden = true;
    nameInput.removeAttribute('required');
  };

  const showRequest = (code: string, client: string, scope: string, resource: string): void => {
    el('#user-code').textContent = code;
    el('#client-id').textContent = client;
    el('#scope').textContent = scope;
    el('#resource').textContent = resource;
    el('#request-details').hidden = false;
    authFlow.hidden = true;
    el('#device-flow').hidden = false;
  };

  const showDevice = async (): Promise<void> => {
    if (!userCode) {
      setError('This device link is missing its user code. Run sigiro auth login again.');
      return;
    }
    if (!(await signedIn())) {
      showAuth();
      return;
    }
    await ensureWorkspace();
    const details = await request<DeviceDetails>(
      `/device?user_code=${encodeURIComponent(userCode)}`,
    );
    showRequest(
      userCode,
      details.client_id ?? 'Unknown client',
      details.scope ?? 'No scopes requested',
      Array.isArray(details.resource)
        ? details.resource.join(', ')
        : (details.resource ?? 'No resource requested'),
    );
    setStatus('Review the request before approving it.');
  };

  const showAgentDevice = async (): Promise<void> => {
    if (!agentId || !agentCode) {
      setError('This agent approval link is incomplete. Start the connection again.');
      return;
    }
    if (!(await signedIn())) {
      showAuth();
      return;
    }
    await ensureWorkspace();
    showRequest(agentCode, agentId, 'Capabilities requested by this agent', location.origin);
    setStatus('Confirm this code matches the requesting agent before approving.');
  };

  const decide = async (
    approve: boolean,
    run: () => Promise<void>,
    failure: string,
  ): Promise<void> => {
    setStatus(approve ? 'Approving…' : 'Denying…');
    try {
      await run();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : failure);
    }
  };

  const decideDevice = async (approve: boolean): Promise<void> =>
    decide(
      approve,
      async () => {
        await request(approve ? '/device/approve' : '/device/deny', {
          method: 'POST',
          body: JSON.stringify({ userCode }),
        });
        showSuccess(
          approve
            ? 'Return to your terminal. The CLI is connected.'
            : 'The device request was denied.',
        );
      },
      'Could not process the device request',
    );

  const decideAgentDevice = async (approve: boolean): Promise<void> =>
    decide(
      approve,
      async () => {
        await request('/agent/approve-capability', {
          method: 'POST',
          body: JSON.stringify({
            agent_id: agentId,
            user_code: agentCode,
            action: approve ? 'approve' : 'deny',
          }),
        });
        showSuccess(
          approve ? 'Return to your agent. Access is approved.' : 'The agent request was denied.',
        );
      },
      'Could not process the agent request',
    );

  const decideConsent = async (accept: boolean): Promise<void> => {
    if (!oauthQuery) {
      setError('This authorization request is missing its signed query.');
      return;
    }
    return decide(
      accept,
      async () => {
        const result = await request<RedirectBody>('/oauth2/consent', {
          method: 'POST',
          body: JSON.stringify({ accept, oauth_query: oauthQuery }),
        });
        location.assign(result.redirect_uri);
      },
      'Could not process consent',
    );
  };

  emailForm.addEventListener('submit', (event) => {
    event.preventDefault();
    email = emailInput.value.trim().toLowerCase();
    setStatus('Sending your code…');
    void request('/email-otp/send-verification-otp', {
      method: 'POST',
      body: JSON.stringify({ email, type: 'sign-in' }),
    })
      .then(() => {
        emailForm.hidden = true;
        otpForm.hidden = false;
        otpInput.focus();
        setStatus('Check your inbox for the six-digit code.');
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : 'Could not send the code');
      });
  });

  otpForm.addEventListener('submit', (event) => {
    event.preventDefault();
    setStatus('Verifying…');
    void (async () => {
      try {
        const body =
          page === 'sign-up'
            ? { email, otp: otpInput.value.trim(), name: nameInput.value.trim() }
            : { email, otp: otpInput.value.trim() };
        await request('/sign-in/email-otp', { method: 'POST', body: JSON.stringify(body) });
        await ensureWorkspace();
        if (page === 'device') return await showDevice();
        if (page === 'agent-device') return await showAgentDevice();
        if (await finishOAuthLogin()) return;
        showSuccess('Your Sigiro account and personal workspace are active.');
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : 'Verification failed');
      }
    })();
  });

  el('#restart').addEventListener('click', () => {
    email = '';
    otpInput.value = '';
    otpForm.hidden = true;
    emailForm.hidden = false;
    emailInput.focus();
    setStatus('');
  });

  const approveButton = el('#approve');
  const denyButton = el('#deny');
  if (page === 'agent-device') {
    approveButton.addEventListener('click', () => void decideAgentDevice(true));
    denyButton.addEventListener('click', () => void decideAgentDevice(false));
  } else {
    approveButton.addEventListener('click', () => void decideDevice(true));
    denyButton.addEventListener('click', () => void decideDevice(false));
  }
  el('#accept-consent').addEventListener('click', () => void decideConsent(true));
  el('#deny-consent').addEventListener('click', () => void decideConsent(false));

  void (async () => {
    try {
      if (page === 'device') return await showDevice();
      if (page === 'agent-device') return await showAgentDevice();
      if (page === 'consent') {
        if (!(await signedIn())) {
          location.assign(`/sign-in?oauth_query=${encodeURIComponent(oauthQuery ?? '')}`);
          return;
        }
        await ensureWorkspace();
        el('#consent-flow').hidden = false;
        return;
      }
      if (await signedIn()) {
        await ensureWorkspace();
        if (await finishOAuthLogin()) return;
        showSuccess('You are signed in and your personal workspace is active.');
        return;
      }
      showAuth();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Authentication failed');
    }
  })();
};

start();
