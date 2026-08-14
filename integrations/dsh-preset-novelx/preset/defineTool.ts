/**
 * Minimal defineTool for a user preset under ~/.dsh/.
 * Do not import @deepseek-ai/dsh-tools here: Node resolves that from this
 * directory and never reaches the harness node_modules.
 */

type ParamSpec = { type: string; description?: string }

export function defineTool(options: {
  name: string
  description: string
  parameters?: Record<string, ParamSpec>
  output: {
    schema: unknown
    render: (args: unknown, value: unknown) => unknown
  }
  timeoutMs?: number
  execute: (args: Record<string, unknown>) => unknown
}) {
  const properties: Record<string, ParamSpec> = {}
  for (const [key, spec] of Object.entries(options.parameters || {})) {
    properties[key] = { type: spec.type, description: spec.description }
  }
  return {
    name: options.name,
    description: options.description,
    parameters: { type: 'object', properties },
    output: options.output,
    ...(options.timeoutMs !== undefined ? { timeoutMs: options.timeoutMs } : {}),
    async execute(args: unknown) {
      return options.execute((args && typeof args === 'object' ? args : {}) as Record<string, unknown>)
    },
  }
}
