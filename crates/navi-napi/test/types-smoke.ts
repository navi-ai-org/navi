import {
  NaviNapiEngineBuilder,
  type HostToolInvocation,
  type HostToolResult,
  type RuntimeEvent,
} from '../index';

const builder = new NaviNapiEngineBuilder('.');
builder.configureLearning({ language: 'pt-BR', maxConsecutiveErrors: 5 });
builder.onToolCall((payload) => {
  console.log(payload.invocation);
});
builder.hostTool(
  {
    name: 'questionario',
    description: 'Gera questoes.',
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
