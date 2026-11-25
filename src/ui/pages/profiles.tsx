import { useEffect, useState } from "react";
import { Link } from "react-router";
import { useTransport } from "../transport/context";
import { ProfileCard } from "./profiles/profile-card";
import type { Profile } from "./profiles/profile-model";

export function ProfilesPage() {
  const transport = useTransport();
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    if (!transport) {
      setLoading(false);
      return;
    }
    transport
      .command<{ items: Profile[] }>("profiles_list", { limit: 24, status: "active" })
      .then((result) => {
        if (active) setProfiles(result.items);
      })
      .catch(() => {
        if (active) setError("PROFILES_UNAVAILABLE");
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [transport]);
  return (
    <div className="profiles-shell">
      <aside className="space-rail" aria-label="Spaces">
        <div className="brand-lockup">
          <span className="brand-mark">M</span>
          <span>Mien</span>
        </div>
        <div className="space-rail__label">SPACES</div>
        <button className="space-item space-item--selected" type="button">
          <span className="identity-orb identity-orb--small" />
          All profiles <span>0</span>
        </button>
        <button className="space-item" type="button">
          <span className="identity-orb identity-orb--small identity-orb--growth" />
          Growth <span>0</span>
        </button>
        <button className="space-item" type="button">
          <span className="identity-orb identity-orb--small identity-orb--qa" />
          QA <span>0</span>
        </button>
        <div className="space-rail__footer">
          <span className="space-rail__label">CONTROL PLANE</span>
          <span className="mono">FAIL-CLOSED</span>
        </div>
      </aside>
      <section className="profiles-main">
        <header className="command-bar">
          <div>
            <p className="eyebrow">SPACE / ALL PROFILES</p>
            <h1>
              <span className="sr-only">档案 </span>Deep Current
            </h1>
            <p className="profiles-subtitle">隔离浏览环境的可见状态与验证证据。</p>
          </div>
          <div className="command-actions">
            <button className="button button--ghost" type="button">
              导入档案
            </button>
            <Link className="button button--primary" to="/profiles/new">
              创建档案
            </Link>
          </div>
        </header>
        <section className="live-sessions" aria-labelledby="live-heading">
          <div className="section-heading">
            <div>
              <p className="eyebrow">RUNTIME SIGNAL</p>
              <h2 id="live-heading">Live sessions</h2>
            </div>
            <span className="live-count">
              {profiles.filter((profile) => profile.runtime?.state === "running").length} RUNNING
            </span>
          </div>
          <div className="live-empty">
            <span className="live-empty__signal" aria-hidden="true" />
            <div>
              <strong>没有正在运行的会话</strong>
              <p>启动档案后，代理线路、延迟、runtime 与 isolation 会在这里持续可见。</p>
            </div>
          </div>
        </section>
        <section className="profile-field" aria-labelledby="profiles-heading">
          <div className="section-heading">
            <div>
              <p className="eyebrow">PROFILE FIELD</p>
              <h2 id="profiles-heading">档案</h2>
            </div>
            <span className="live-count">{profiles.length} TOTAL</span>
          </div>
          {loading ? (
            <div className="profile-grid">
              <span className="profile-skeleton" />
              <span className="profile-skeleton" />
              <span className="profile-skeleton" />
            </div>
          ) : profiles.length > 0 ? (
            <div className="profile-grid">
              {profiles.map((profile) => (
                <ProfileCard key={profile.id} profile={profile} />
              ))}
            </div>
          ) : (
            <div className="profiles-empty">
              <span className="empty-orb" aria-hidden="true" />
              <div>
                <h3>还没有档案</h3>
                <p>
                  {error
                    ? `暂时无法读取服务端（${error}）。你仍可以先创建一个档案。`
                    : "创建第一个隔离浏览档案，开始建立可验证的环境。"}
                </p>
                <Link className="button button--primary" to="/profiles/new">
                  创建第一个档案
                </Link>
              </div>
            </div>
          )}
        </section>
      </section>
    </div>
  );
}
