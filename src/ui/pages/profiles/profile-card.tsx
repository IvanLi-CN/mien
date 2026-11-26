import { useCallback, useEffect, useState } from "react";
import { useTransport } from "../../transport/context";
import type { ControlTransport } from "../../transport/types";
import type { Profile, RuntimeStart, RuntimeStatus } from "./profile-model";
import {
  isolationLabels,
  modeLabels,
  profileState,
  proofTone,
  runtimeStateClasses,
  runtimeStateLabels,
} from "./profile-model";

export function ProfileStatus({ state }: { state: keyof typeof runtimeStateLabels }) {
  return (
    <span className={runtimeStateClasses[state]}>
      <span className="sr-only">状态：</span>
      <span className="profile-status__dot" aria-hidden="true" />
      {runtimeStateLabels[state]}
    </span>
  );
}
export function IsolationProof({ isolation }: { isolation: Profile["isolation"] }) {
  const tone =
    isolation === "enforced" ? "verified" : isolation === "unsandboxed" ? "warning" : "unknown";
  return (
    <span className={proofTone(tone)} title={isolationLabels[isolation]}>
      <span className="proof-segment__mark" aria-hidden="true">
        {isolation === "unknown" ? "?" : ""}
      </span>
      <span>IS · {isolationLabels[isolation]}</span>
    </span>
  );
}
export function ProofStrip({
  profile,
  state,
  isolation,
}: {
  profile: Profile;
  state: keyof typeof runtimeStateLabels;
  isolation: Profile["isolation"];
}) {
  const runtimeTone =
    state === "running"
      ? "verified"
      : state === "crashed" || state === "failed"
        ? "failed"
        : "neutral";
  const proxyTone = profile.direct ? "warning" : "verified";
  return (
    <div className="proof-strip">
      <span className={proofTone("verified")}>
        <span className="proof-segment__mark" aria-hidden="true" />
        FP · 指纹
      </span>
      <span className={proofTone(proxyTone)}>
        <span className="proof-segment__mark" aria-hidden="true" />
        PX · {profile.direct ? "直连" : "代理"}
      </span>
      <span className={proofTone(runtimeTone)}>
        <span className="proof-segment__mark" aria-hidden="true" />
        RT · {runtimeStateLabels[state]}
      </span>
      <IsolationProof isolation={isolation} />
    </div>
  );
}

/** 命令失败是 `{ code, message }`；其他形状回退到通用文案。 */
export function commandErrorText(error: unknown, fallback: string): string {
  if (error && typeof error === "object") {
    const payload = error as { code?: unknown; message?: unknown };
    if (typeof payload.code === "string") {
      return typeof payload.message === "string" && payload.message
        ? `${payload.code}：${payload.message}`
        : payload.code;
    }
  }
  return fallback;
}

export function ProfileCard({ profile }: { profile: Profile }) {
  const transport = useTransport();
  const [status, setStatus] = useState<RuntimeStatus | null>(null);
  const [disclosures, setDisclosures] = useState<string[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(
    async (active: () => boolean = () => true) => {
      if (!transport) return;
      try {
        const result = await transport.command<RuntimeStatus>("runtime_status", {
          profileId: profile.id,
        });
        if (active()) setStatus(result);
      } catch {
        // 状态读取失败不阻断卡片；保留已知状态并让启动/停止按钮继续可用。
      }
    },
    [transport, profile.id],
  );

  useEffect(() => {
    let active = true;
    void refresh(() => active);
    return () => {
      active = false;
    };
  }, [refresh]);

  const state = status?.state ?? profileState(profile);
  const isolation = status?.isolation ?? profile.isolation;
  const running = state === "running";
  const busy = pending || state === "starting" || state === "stopping";

  const start = async () => {
    if (!transport || pending) return;
    setPending(true);
    setError(null);
    try {
      const result = await transport.command<RuntimeStart>("runtime_start", {
        profileId: profile.id,
      });
      setStatus({
        state: result.state,
        isolation: result.isolation,
        proxyPort: result.proxyPort,
        cdpPort: result.cdpPort,
      } as RuntimeStatus);
      setDisclosures(result.disclosures ?? []);
    } catch (failure) {
      setError(commandErrorText(failure, "启动失败"));
      await refresh();
    } finally {
      setPending(false);
    }
  };
  const stop = async () => {
    if (!transport || pending) return;
    setPending(true);
    setError(null);
    try {
      const result = await transport.command<RuntimeStatus>("runtime_stop", {
        profileId: profile.id,
      });
      setStatus(result);
      setDisclosures([]);
    } catch (failure) {
      setError(commandErrorText(failure, "停止失败"));
      await refresh();
    } finally {
      setPending(false);
    }
  };

  return (
    <article className="profile-card" data-state={state}>
      <div className="identity-orb" aria-hidden="true" />
      <div className="profile-card__header">
        <div>
          <p className="eyebrow">{profile.group_id ?? "ALL PROFILES"}</p>
          <h3>{profile.name}</h3>
        </div>
        <ProfileStatus state={state} />
      </div>
      <dl className="profile-meta">
        <div>
          <dt>MODE</dt>
          <dd>{modeLabels[profile.mode]}</dd>
        </div>
        <div>
          <dt>KERNEL</dt>
          <dd>{profile.kernel_version}</dd>
        </div>
        <div>
          <dt>PROXY</dt>
          <dd>{profile.proxy_id ? "已配置" : "直连"}</dd>
        </div>
      </dl>
      <ProofStrip profile={profile} state={state} isolation={isolation} />
      {profile.limitations.length > 0 && (
        <p className="profile-card__warning">{profile.limitations[0]}</p>
      )}
      {disclosures.length > 0 && (
        <ul className="profile-card__disclosures" aria-label="启动声明">
          {disclosures.map((disclosure) => (
            <li key={disclosure}>{disclosure}</li>
          ))}
        </ul>
      )}
      {error && (
        <p className="profile-card__warning" role="alert">
          {error}
        </p>
      )}
      <div className="profile-card__actions">
        <a className="button button--quiet" href={`/profiles/${profile.id}`}>
          打开档案
        </a>
        <RuntimeButton
          running={running}
          busy={busy}
          onStart={start}
          onStop={stop}
          transport={transport}
        />
      </div>
    </article>
  );
}

function RuntimeButton({
  running,
  busy,
  onStart,
  onStop,
  transport,
}: {
  running: boolean;
  busy: boolean;
  onStart: () => Promise<void>;
  onStop: () => Promise<void>;
  transport: ControlTransport | null;
}) {
  return (
    <button
      className="button button--ghost"
      type="button"
      disabled={busy || !transport}
      onClick={running ? onStop : onStart}
    >
      {busy ? "处理中…" : running ? "停止" : "启动"}
    </button>
  );
}
