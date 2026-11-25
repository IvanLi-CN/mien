import { useEffect, useState } from "react";
import { Link, useParams } from "react-router";
import { useTransport } from "../transport/context";
import { ProfileForm } from "./profile-edit/profile-form";
import { IsolationProof, ProfileStatus, ProofStrip } from "./profiles/profile-card";
import { modeLabels, type Profile, profileState } from "./profiles/profile-model";

export function ProfileNewPage() {
  return <ProfileForm />;
}
export function ProfileDetailPage({ edit = false }: { edit?: boolean }) {
  const { id } = useParams<{ id: string }>();
  return edit ? <ProfileForm profileId={id} /> : <ProfileDetail id={id} />;
}
function ProfileDetail({ id }: { id?: string }) {
  const transport = useTransport();
  const [profile, setProfile] = useState<Profile | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!id || !transport) return;
    transport
      .command<Profile>("profiles_get", { profile_id: id })
      .then(setProfile)
      .catch(() => setError("PROFILE_UNAVAILABLE"));
  }, [id, transport]);
  if (error)
    return (
      <section className="detail-page">
        <p className="form-error" role="alert">
          无法读取档案：{error}
        </p>
        <Link className="button button--quiet" to="/profiles">
          回到档案
        </Link>
      </section>
    );
  if (!profile)
    return (
      <section className="detail-page">
        <h1 className="sr-only">档案</h1>
        <span className="profile-skeleton" />
      </section>
    );
  const state = profileState(profile);
  return (
    <section className="detail-page">
      <header className="detail-heading">
        <div>
          <p className="eyebrow">PROFILE / {profile.id}</p>
          <h1>{profile.name}</h1>
          <div className="detail-status">
            <ProfileStatus state={state} />
            <span>{modeLabels[profile.mode]}</span>
            <IsolationProof isolation={profile.isolation} />
          </div>
        </div>
        <div className="command-actions">
          <Link className="button button--quiet" to={`/profiles/${profile.id}/edit`}>
            编辑
          </Link>
          <Link className="button button--ghost" to="/profiles">
            回到档案
          </Link>
        </div>
      </header>
      <div className="detail-grid">
        <article className="detail-panel">
          <p className="eyebrow">VERIFICATION</p>
          <h2>Proof</h2>
          <ProofStrip profile={profile} />
          <dl className="detail-list">
            <div>
              <dt>代理</dt>
              <dd>{profile.direct ? "直连 / 降低保证" : (profile.proxy_id ?? "已配置")}</dd>
            </div>
            <div>
              <dt>内核</dt>
              <dd>{profile.kernel_version}</dd>
            </div>
            <div>
              <dt>限制</dt>
              <dd>{profile.limitations.length ? profile.limitations.join("、") : "无额外限制"}</dd>
            </div>
          </dl>
        </article>
        <article className="detail-panel">
          <p className="eyebrow">RUNTIME</p>
          <h2>运行时</h2>
          <p className="detail-runtime-state">
            <ProfileStatus state={state} />
          </p>
          <p className="form-lede">
            宿主绑定、字体、plugins 与运行模式限制在这里保持可见；凭据永不回显。
          </p>
        </article>
      </div>
    </section>
  );
}
