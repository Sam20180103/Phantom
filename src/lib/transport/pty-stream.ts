import { transport } from "@/lib/transport/client";
import type { PtyStreamTransport } from "@/modules/terminal/lib/PtyOutputHub";
import { tokenBytes } from "@/modules/terminal/lib/ptyWire";

export function webSocketPtyTransport(): PtyStreamTransport {
  return {
    open: async () =>
      tokenBytes(await transport.invoke<string>("pty_stream_open", {})),
    poll: (body, signal) => transport.ptyStreamRequest("poll", body, signal),
    ack: (body) => transport.ptyStreamRequest("ack", body),
  };
}
