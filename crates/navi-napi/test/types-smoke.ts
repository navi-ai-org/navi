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

// ATIF export surface is typed through the .d.ts boundary.
async function exportAtif(): Promise<string> {
  return engine.exportSessionAtif('saved-session-id', true);
}
void exportAtif();

void readFirstEvent();
