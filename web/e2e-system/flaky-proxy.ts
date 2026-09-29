import net from "node:net";

/**
 * A TCP forwarder in front of the app that can cut every connection and refuse new ones: a
 * network drop the browser really sees, on the connections it really holds open (the SSE
 * stream). Playwright's `context.setOffline` does not do this: Chromium keeps an already
 * established event stream alive when the emulated network goes offline.
 */
export class FlakyProxy {
  private readonly sockets = new Set<net.Socket>();
  private blocked = false;
  private readonly server: net.Server;

  constructor(private readonly target: { host: string; port: number }) {
    this.server = net.createServer((client) => {
      if (this.blocked) {
        client.destroy();
        return;
      }
      const upstream = net.connect(this.target.port, this.target.host);
      for (const socket of [client, upstream]) {
        this.sockets.add(socket);
        socket.on("error", () => socket.destroy());
        socket.on("close", () => {
          this.sockets.delete(socket);
          // one side is gone: the other must not linger half-open
          (socket === client ? upstream : client).destroy();
        });
      }
      client.pipe(upstream);
      upstream.pipe(client);
    });
  }

  async start(port: number): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      this.server.once("error", reject);
      this.server.listen(port, "127.0.0.1", resolve);
    });
  }

  /** Cuts every open connection and refuses new ones until `unblock`. */
  block(): void {
    this.blocked = true;
    for (const socket of this.sockets) socket.destroy();
  }

  unblock(): void {
    this.blocked = false;
  }

  async stop(): Promise<void> {
    this.block();
    await new Promise<void>((resolve) => this.server.close(() => resolve()));
  }
}
