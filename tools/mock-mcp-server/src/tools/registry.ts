export interface CannedResponse {
  key: string;
  label: string;
  isError: boolean;
  content: Array<{ type: 'text'; text: string }>;
}

export interface MockToolDef {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
  responses: CannedResponse[];
  defaultResponseKey: string;
}

const weatherTool: MockToolDef = {
  name: 'get_weather',
  description: 'Get current weather for a city',
  inputSchema: {
    type: 'object',
    properties: {
      city: { type: 'string', description: 'City name' },
      units: { type: 'string', enum: ['celsius', 'fahrenheit'], description: 'Temperature units' },
    },
    required: ['city'],
  },
  responses: [
    {
      key: 'sunny',
      label: 'Sunny weather',
      isError: false,
      content: [{ type: 'text', text: JSON.stringify({ city: '{{city}}', temperature: 24, condition: 'sunny', humidity: 45, wind: '12 km/h NW' }, null, 2) }],
    },
    {
      key: 'rainy',
      label: 'Rainy weather',
      isError: false,
      content: [{ type: 'text', text: JSON.stringify({ city: '{{city}}', temperature: 12, condition: 'rainy', humidity: 89, wind: '25 km/h SE' }, null, 2) }],
    },
    {
      key: 'error',
      label: 'City not found',
      isError: true,
      content: [{ type: 'text', text: 'Error: City "{{city}}" not found. Please check the city name and try again.' }],
    },
    {
      key: 'timeout',
      label: 'Service timeout',
      isError: true,
      content: [{ type: 'text', text: 'Error: Weather service timed out after 30 seconds. Please try again later.' }],
    },
  ],
  defaultResponseKey: 'sunny',
};

const databaseTool: MockToolDef = {
  name: 'search_database',
  description: 'Search records in a database',
  inputSchema: {
    type: 'object',
    properties: {
      query: { type: 'string', description: 'Search query' },
      table: { type: 'string', description: 'Table name' },
      limit: { type: 'number', description: 'Max results to return' },
    },
    required: ['query'],
  },
  responses: [
    {
      key: 'results',
      label: 'Results found',
      isError: false,
      content: [{ type: 'text', text: JSON.stringify({ total: 3, rows: [{ id: 1, name: 'Alice', email: 'alice@example.com' }, { id: 2, name: 'Bob', email: 'bob@example.com' }, { id: 3, name: 'Charlie', email: 'charlie@example.com' }] }, null, 2) }],
    },
    {
      key: 'empty',
      label: 'No results',
      isError: false,
      content: [{ type: 'text', text: JSON.stringify({ total: 0, rows: [] }, null, 2) }],
    },
    {
      key: 'error',
      label: 'Query error',
      isError: true,
      content: [{ type: 'text', text: 'Error: Syntax error in query near "{{query}}". Check your query syntax.' }],
    },
  ],
  defaultResponseKey: 'results',
};

const emailTool: MockToolDef = {
  name: 'send_email',
  description: 'Send an email message',
  inputSchema: {
    type: 'object',
    properties: {
      to: { type: 'string', description: 'Recipient email address' },
      subject: { type: 'string', description: 'Email subject' },
      body: { type: 'string', description: 'Email body' },
    },
    required: ['to', 'subject', 'body'],
  },
  responses: [
    {
      key: 'success',
      label: 'Email sent',
      isError: false,
      content: [{ type: 'text', text: JSON.stringify({ status: 'sent', messageId: 'msg-abc123', to: '{{to}}', timestamp: '{{__now}}' }, null, 2) }],
    },
    {
      key: 'bounced',
      label: 'Email bounced',
      isError: true,
      content: [{ type: 'text', text: 'Error: Email to "{{to}}" bounced. The address does not exist or the mailbox is full.' }],
    },
    {
      key: 'rate_limited',
      label: 'Rate limited',
      isError: true,
      content: [{ type: 'text', text: 'Error: Rate limit exceeded. You can send a maximum of 100 emails per hour. Try again in 15 minutes.' }],
    },
  ],
  defaultResponseKey: 'success',
};

const calculatorTool: MockToolDef = {
  name: 'calculate',
  description: 'Evaluate a math expression',
  inputSchema: {
    type: 'object',
    properties: {
      expression: { type: 'string', description: 'Mathematical expression to evaluate' },
    },
    required: ['expression'],
  },
  responses: [
    {
      key: 'result',
      label: 'Successful calculation',
      isError: false,
      content: [{ type: 'text', text: JSON.stringify({ expression: '{{expression}}', result: 42, type: 'number' }, null, 2) }],
    },
    {
      key: 'division_by_zero',
      label: 'Division by zero',
      isError: true,
      content: [{ type: 'text', text: 'Error: Division by zero in expression "{{expression}}".' }],
    },
    {
      key: 'overflow',
      label: 'Numeric overflow',
      isError: true,
      content: [{ type: 'text', text: 'Error: Numeric overflow. The result of "{{expression}}" exceeds the maximum representable value.' }],
    },
  ],
  defaultResponseKey: 'result',
};

const filesTool: MockToolDef = {
  name: 'file_operations',
  description: 'Read, write, or list files',
  inputSchema: {
    type: 'object',
    properties: {
      operation: { type: 'string', enum: ['read', 'write', 'list'], description: 'File operation type' },
      path: { type: 'string', description: 'File or directory path' },
      content: { type: 'string', description: 'Content to write (for write operation)' },
    },
    required: ['operation', 'path'],
  },
  responses: [
    {
      key: 'content',
      label: 'File content',
      isError: false,
      content: [{ type: 'text', text: JSON.stringify({ path: '{{path}}', content: '# Example File\n\nThis is mock file content returned by the mock MCP server.\nLine 3 of the file.\n', size: 89, modified: '{{__now}}' }, null, 2) }],
    },
    {
      key: 'listing',
      label: 'Directory listing',
      isError: false,
      content: [{ type: 'text', text: JSON.stringify({ path: '{{path}}', entries: [{ name: 'README.md', type: 'file', size: 1024 }, { name: 'src', type: 'directory' }, { name: 'package.json', type: 'file', size: 512 }] }, null, 2) }],
    },
    {
      key: 'write_success',
      label: 'Write success',
      isError: false,
      content: [{ type: 'text', text: JSON.stringify({ path: '{{path}}', status: 'written', bytesWritten: 256 }, null, 2) }],
    },
    {
      key: 'not_found',
      label: 'File not found',
      isError: true,
      content: [{ type: 'text', text: 'Error: File "{{path}}" not found. Check the path and try again.' }],
    },
    {
      key: 'permission_denied',
      label: 'Permission denied',
      isError: true,
      content: [{ type: 'text', text: 'Error: Permission denied for "{{path}}". Check file permissions.' }],
    },
  ],
  defaultResponseKey: 'content',
};

export const ALL_TOOLS: MockToolDef[] = [
  weatherTool,
  databaseTool,
  emailTool,
  calculatorTool,
  filesTool,
];

export function getToolDef(name: string): MockToolDef | undefined {
  return ALL_TOOLS.find(t => t.name === name);
}

export function resolveResponse(
  tool: MockToolDef,
  responseKey: string | null,
  args: Record<string, unknown>
): { isError: boolean; content: Array<{ type: 'text'; text: string }> } {
  const key = responseKey ?? tool.defaultResponseKey;
  const resp = tool.responses.find(r => r.key === key) ?? tool.responses[0];

  const content = resp.content.map(c => ({
    type: c.type as 'text',
    text: interpolateTemplate(c.text, args),
  }));

  return { isError: resp.isError, content };
}

function interpolateTemplate(template: string, args: Record<string, unknown>): string {
  return template.replace(/\{\{(\w+)\}\}/g, (_match, key) => {
    if (key === '__now') return new Date().toISOString();
    const val = args[key];
    return val !== undefined ? String(val) : `{{${key}}}`;
  });
}
