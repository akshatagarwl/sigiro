interface Env {
  WAITLIST: KVNamespace
}

const EMAIL_RE = /^[^@\s]+@[^@\s]+\.[^@\s]+$/

export const onRequestPost: PagesFunction<Env> = async ({ request, env }) => {
  const origin = 'https://sigiro.com'
  const headers = { 'Content-Type': 'application/json', 'Access-Control-Allow-Origin': origin }

  if (!env.WAITLIST) {
    return new Response(JSON.stringify({ error: 'KV not bound' }), { status: 500, headers })
  }

  // Rate limit: 5 submissions per IP per hour
  const ip = request.headers.get('CF-Connecting-IP') ?? 'unknown'
  const rateKey = `rate:${ip}`
  const rateVal = await env.WAITLIST.get(rateKey)
  const count = rateVal ? parseInt(rateVal, 10) : 0
  if (count >= 5) {
    return new Response(JSON.stringify({ error: 'rate limited' }), { status: 429, headers })
  }
  await env.WAITLIST.put(rateKey, String(count + 1), { expirationTtl: 3600 })

  let body: { email?: string; source?: string }
  try {
    body = await request.json()
  } catch {
    return new Response(JSON.stringify({ error: 'invalid json' }), { status: 400, headers })
  }

  const email = body.email?.trim().toLowerCase()
  if (!email || !EMAIL_RE.test(email) || email.length > 320) {
    return new Response(JSON.stringify({ error: 'invalid email' }), { status: 400, headers })
  }

  const value = JSON.stringify({
    email,
    source: body.source ?? 'unknown',
    timestamp: new Date().toISOString(),
    country: request.headers.get('CF-IPCountry') ?? 'unknown',
  })

  await env.WAITLIST.put(`email:${email}`, value)

  return new Response(JSON.stringify({ ok: true }), { status: 200, headers })
}

export const onRequestOptions: PagesFunction = async () => {
  return new Response(null, {
    headers: {
      'Access-Control-Allow-Origin': 'https://sigiro.com',
      'Access-Control-Allow-Methods': 'POST, OPTIONS',
      'Access-Control-Allow-Headers': 'Content-Type',
    },
  })
}
