import type { Profile } from "./profile-model";
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
export function ProofStrip({ profile }: { profile: Profile }) {
  const state = profileState(profile);
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
      <IsolationProof isolation={profile.isolation} />
    </div>
  );
}
export function ProfileCard({ profile }: { profile: Profile }) {
  const state = profileState(profile);
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
      <ProofStrip profile={profile} />
      {profile.limitations.length > 0 && (
        <p className="profile-card__warning">{profile.limitations[0]}</p>
      )}
      <div className="profile-card__actions">
        <a className="button button--quiet" href={`/profiles/${profile.id}`}>
          打开档案
        </a>
        <button
          className="button button--ghost"
          type="button"
          disabled={state === "stopping" || state === "starting"}
        >
          {state === "running" ? "停止" : "启动"}
        </button>
      </div>
    </article>
  );
}
