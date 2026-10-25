/**
 * The MCP server, as the Keys & network page offers it to an agent: the
 * setting that lets agents in, the address one is given, the name a key made
 * for one is issued under, and the line that connects Claude Code.
 */

/** The setting that lets agents in. */
export const MCP_SETTING = 'mcp.enabled'

/** Where the MCP server answers, under a base address with or without its slash. */
export function mcpAddress(base: string): string {
  return `${base.replace(/\/+$/, '')}/mcp`
}

/**
 * The first name for an agent's key that none of the server's own keys holds
 * yet — `Agent`, `Agent 2`, `Agent 3`… — compared as the server compares
 * names: without regard to case, around their spaces.
 */
export function freeAgentName(base: string, taken: readonly string[]): string {
  const held = new Set(taken.map((name) => name.trim().toLowerCase()))
  for (let n = 1; ; n++) {
    const name = n === 1 ? base : `${base} ${n}`
    if (!held.has(name.toLowerCase())) return name
  }
}

/**
 * What Claude Code is told to connect with: the address over HTTP, and the
 * key in a header — `<key>` until one is made.
 */
export function claudeCommand(address: string, key: string | null): string {
  return `claude mcp add --transport http arr-metadata ${address} --header "Authorization: Bearer ${key ?? '<key>'}"`
}
