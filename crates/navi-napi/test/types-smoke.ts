import {
  NaviNapiEngineBuilder,
  type HostToolInvocation,
  type HostToolResult,
  type RuntimeEvent,
} from '../index';

const builder = new NaviNapiEngineBuilder('.');
builder.onToolCall((payload) => {
  console.log(payload.invocation);
});
builder.hostTool(
  {
    name: 'lookup_docs',
    description: 'Look up documentation.',
    kind: 'read',
    inputSchema: { type: 'object' },
  },
  async (invocation: HostToolInvocation): Promise<HostToolResult> => ({
    ok: true,
    output: { invocationId: invocation.invocationId },
  }),
);

const engine = builder.build();
async function readFirstEvent(): Promise<RuntimeEvent | null> {
  const session = await engine.startSession();
  return engine.subscribeEvents(session.id).next();
}

void readFirstEvent();
