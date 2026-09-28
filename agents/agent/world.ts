import { createWorld as createRivetWorld } from "@rivet-dev/vercel-world";

import { registry } from "./actors.ts";

export const createWorld = () => createRivetWorld({ registry });
