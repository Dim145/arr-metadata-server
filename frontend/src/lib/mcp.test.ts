import { describe, expect, it } from 'vitest'

import { claudeCommand, freeAgentName, mcpAddress } from './mcp'

describe('the MCP server, as an agent is given it', () => {
  it('answers under /mcp, whatever slash the base address ends with', () => {
    expect(mcpAddress('https://ams.example.com')).toBe('https://ams.example.com/mcp')
    expect(mcpAddress('https://ams.example.com/')).toBe('https://ams.example.com/mcp')
    expect(mcpAddress('http://127.0.0.1:8479//')).toBe('http://127.0.0.1:8479/mcp')
  })

  it('names a key for an agent with the first name no key holds', () => {
    expect(freeAgentName('Agent', [])).toBe('Agent')
    expect(freeAgentName('Agent', ['Sonarr', 'agent '])).toBe('Agent 2')
    expect(freeAgentName('Agent', ['Agent', 'AGENT 2', 'Agent 4'])).toBe('Agent 3')
  })

  it('tells Claude Code the address and the key, in a header', () => {
    expect(claudeCommand('https://ams.example.com/mcp', null)).toBe(
      'claude mcp add --transport http arr-metadata https://ams.example.com/mcp --header "Authorization: Bearer <key>"',
    )
    expect(claudeCommand('https://ams.example.com/mcp', 'ams_abc')).toContain('Bearer ams_abc"')
  })
})
