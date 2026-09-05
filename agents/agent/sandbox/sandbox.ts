import { defineSandbox } from "eve/sandbox";
import { docker } from "eve/sandbox/docker";

export default defineSandbox({
  backend: () => {
    const image = process.env.EVE_SANDBOX_IMAGE;
    if (!image) throw new Error("EVE_SANDBOX_IMAGE is required and must contain the repository's offline build dependencies");
    return docker({ image, networkPolicy: "deny-all", pullPolicy: "never" });
  },
});
