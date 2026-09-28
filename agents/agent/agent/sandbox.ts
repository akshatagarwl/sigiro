import { defineSandbox } from "eve/sandbox";
import { cloudflareSandbox } from "../optimizer/cloudflare.ts";

export default defineSandbox({ backend: cloudflareSandbox() });
