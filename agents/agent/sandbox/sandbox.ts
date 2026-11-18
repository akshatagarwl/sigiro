import { defineSandbox } from "eve/sandbox";
import { microsandbox } from "eve/sandbox/microsandbox";

const selected = process.env.EVE_SANDBOX_PROVIDER;
if (selected !== "microsandbox") {
  throw new Error(
    `Unsupported sandbox provider: ${selected ?? "none"}; microsandbox is required`,
  );
}

export default defineSandbox({
  backend: microsandbox({
    cpus: 1,
    memoryMiB: 1024,
    setup: { autoInstall: true },
  }),
});
