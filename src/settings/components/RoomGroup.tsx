import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { usePreferencesStore } from "@/modules/settings/preferences";
import { setRoomHubUrl } from "@/modules/settings/store";
import { getRoomToken, setRoomToken } from "@/modules/room/lib/credential";
import { useRoomStore } from "@/modules/room";

/**
 * Where the room lives, for a client that is not served by the hub.
 *
 * Leave the address empty and the room is whatever server delivered this
 * window — right for a browser opened on the hub itself, and the only thing
 * that ever worked before. The desktop app is served by nobody, so it has to be
 * told.
 */
export function RoomGroup() {
  const savedUrl = usePreferencesStore((s) => s.roomHubUrl);
  const hub = useRoomStore((s) => s.hub);
  const unavailable = useRoomStore((s) => s.unavailable);
  const reconnect = useRoomStore((s) => s.reconnect);

  const [url, setUrl] = useState(savedUrl);
  const [token, setToken] = useState("");
  const [hasToken, setHasToken] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => setUrl(savedUrl), [savedUrl]);
  useEffect(() => {
    void getRoomToken().then((t) => setHasToken(!!t));
  }, []);

  const apply = async () => {
    setBusy(true);
    try {
      await setRoomHubUrl(url);
      // An untouched field must not wipe a stored token — only an explicit
      // edit changes it, and clearing the box is how you remove it.
      if (token) {
        await setRoomToken(token);
        setToken("");
        setHasToken(true);
      }
      await reconnect();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-2">
      <Label>Room</Label>
      <p className="text-xs text-muted-foreground">
        The shared log you and your agents post into. Leave the address empty to
        use the room served by this machine.
      </p>

      <div className="flex flex-col gap-1.5">
        <Input
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://192.0.2.5:8099"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
        />
        <Input
          type="password"
          value={token}
          onChange={(e) => setToken(e.target.value)}
          placeholder={
            hasToken ? "token stored — type to replace" : "client token"
          }
          spellCheck={false}
        />
        <div className="flex items-center gap-2">
          <Button size="sm" disabled={busy} onClick={() => void apply()}>
            {busy ? "Connecting…" : "Save and connect"}
          </Button>
          <span className="text-xs text-muted-foreground">
            {unavailable
              ? unavailable
              : hub
                ? `connected to ${hub}`
                : "using this machine's room"}
          </span>
        </div>
      </div>

      <p className="text-xs text-muted-foreground">
        The token comes from the hub's <code>room.client-tokens</code>. It reaches
        the room only — not a shell, not your files. If the hub serves a
        self-signed certificate, add it to this machine's trust store first: a
        webview cannot offer you the "proceed anyway" page a browser does.
      </p>
    </div>
  );
}
