import { eveChannel } from "eve/channels/eve";
import { localDev, type AuthFn, withAuthChallenges } from "eve/channels/auth";
import { authenticateWorker } from "../lib/celld-dispatch.ts";

const workerAuth: AuthFn<Request> = withAuthChallenges((request) => {
  const token = process.env.EVE_WORKER_TOKEN;
  if (token === undefined || !authenticateWorker(request.headers.get("authorization"), token)) return null;
  return {
    authenticator: "eve-worker-token",
    principalId: "celld-worker",
    principalType: "worker",
    attributes: {},
  };
}, [{ scheme: "Bearer" }]);

export default eveChannel({ auth: [workerAuth, localDev()] });
