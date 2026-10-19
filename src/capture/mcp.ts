import { existsSync, readFileSync } from "node:fs";
import type { McpServer, Target } from "../types.js";

/** User-scope MCP servers: the top-level `mcpServers` of the Claude JSON file, and nothing else from it. */
export function readUserMcp(t: Target): Record<string, McpServer> {
  if (!existsSync(t.claudeJson)) return {};
  try {
    const j = JSON.parse(readFileSync(t.claudeJson, "utf8"));
    return j && typeof j.mcpServers === "object" && j.mcpServers && !Array.isArray(j.mcpServers) ? { ...j.mcpServers } : {};
  } catch {
    return {};
  }
}
