import { eveChannel } from "eve/channels/eve";
import { extractBearerToken, verifyJwtHmac, type AuthFn } from "eve/channels/auth";

const serviceAuth: AuthFn = async (request) => {
  const secret = process.env.SIGIRO_OPTIMIZER_EVE_HMAC_SECRET;
  if (!secret || secret.length < 32) return null;
  const result = await verifyJwtHmac(extractBearerToken(request.headers.get("authorization")), {
    algorithm: "HS256",
    issuer: "sigiro-optimizer",
    audiences: ["sigiro-eve"],
    secret,
    clockSkewSeconds: 0,
  });
  return result.ok ? result.sessionAuth : null;
};

export default eveChannel({ auth: [serviceAuth] });
