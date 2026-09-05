import { defineSandbox } from "eve/sandbox";
import { boxSandbox } from "../lib/box-sandbox.ts";

export default defineSandbox({
  backend: () => boxSandbox({ ttlSeconds: 3600 }),
});
