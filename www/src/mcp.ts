import docsData from 'blume:mcp-data';
import { MCP_TOOLS } from 'blume/ai/mcp/tools.ts';
import { createMcpFetchHandler } from 'blume/ai/mcp/server.ts';
import { createMcpHandler, tools } from './mcp-handler';

const docsHandler = createMcpFetchHandler(docsData);
export const docsTools = MCP_TOOLS;
export { tools };
export const handleDocsMcp = docsHandler;
export const handleMcp = createMcpHandler(docsHandler);
