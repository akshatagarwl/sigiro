interface Env {
  WAITLIST: KVNamespace
}

export const onRequestPost: PagesFunction<Env> = async ({ request, env }) => {
  const headers = { 'Content-Type': 'application/json', 'Access-Control-Allow-Origin': '*' }

  let body: { email?: string; source?: string }
  try {
    body = await request.json()
  } catch {
    return new Response(JSON.stringify({ error: 'invalid json' }), { status: 400, headers })
  }

  const email = body.email?.trim().toLowerCase()
  if (!email || !email.includes('@') || email.length > 320) {
    return new Response(JSON.stringify({ error: 'invalid email' }), { status: 400, headers })
  }

  if (!env.WAITLIST) {
    return new Response(JSON.stringify({ error: 'KV not bound' }), { status: 500, headers })
  }

  const value = JSON.stringify({
    email,
    source: body.source ?? 'unknown',
    timestamp: new Date().toISOString(),
    ip: request.headers.get('CF-Connecting-IP') ?? 'unknown',
    country: request.headers.get('CF-IPCountry') ?? 'unknown',
  })

  await env.WAITLIST.put(`email:${email}`, value)

  return new Response(JSON.stringify({ ok: true }), { status: 200, headers })
}

export const onRequestOptions: PagesFunction = async () => {
  return new Response(null, {
    headers: {
      'Access-Control-Allow-Origin': '*',
      'Access-Control-Allow-Methods': 'POST, OPTIONS',
      'Access-Control-Allow-Headers': 'Content-Type',
    },
  })
}
