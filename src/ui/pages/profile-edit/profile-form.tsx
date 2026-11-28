import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { useTransport } from "../../transport/context";
import {
  defaultProfileInput,
  type Profile,
  type ProfileInput,
  type ProfilePatch,
  type RuntimeMode,
} from "../profiles/profile-model";

const modeCopy: Record<RuntimeMode, { label: string; description: string }> = {
  headful: { label: "有头", description: "正常窗口；桌面使用宿主窗口，服务端使用 Xvfb。" },
  headless: { label: "无头", description: "Chromium headless；降低保证，不承诺 UA 结果。" },
  pseudo_headful: {
    label: "伪有头",
    description: "仅 UA 伪装，不是有头等价；其他 Headless 特征保留。",
  },
};

export function ProfileForm({ profile, profileId }: { profile?: Profile; profileId?: string }) {
  const navigate = useNavigate();
  const transport = useTransport();
  const [values, setValues] = useState<ProfileInput>(() => profileToInput(profile));
  const [savedValues, setSavedValues] = useState(() => JSON.stringify(values));
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);
  const dirty = JSON.stringify(values) !== savedValues;

  useEffect(() => {
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      if (!dirty) return;
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => window.removeEventListener("beforeunload", handleBeforeUnload);
  }, [dirty]);

  const update = <K extends keyof ProfileInput>(key: K, value: ProfileInput[K]) => {
    setValues((current) => ({ ...current, [key]: value }));
    setErrors((current) => {
      const next = { ...current };
      delete next[key];
      return next;
    });
  };
  const updateFingerprint = <K extends keyof ProfileInput["fingerprint"]>(
    key: K,
    value: ProfileInput["fingerprint"][K],
  ) =>
    setValues((current) => ({ ...current, fingerprint: { ...current.fingerprint, [key]: value } }));
  const updateWindow = (key: "width" | "height", value: number) =>
    setValues((current) => ({
      ...current,
      fingerprint: {
        ...current.fingerprint,
        window: { ...current.fingerprint.window, [key]: value },
      },
    }));

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (submitting) return;
    if (!values.name.trim()) {
      setErrors({ name: "请输入档案名称" });
      nameRef.current?.focus();
      return;
    }
    setSubmitting(true);
    setSubmitError(null);
    try {
      const result = profileId
        ? await transport?.command<Profile>("profiles_update", {
            id: profileId,
            patch: toPatch(values),
          })
        : await transport?.command<Profile>("profiles_create", { input: toCreateArgs(values) });
      if (!result) throw new Error("传输层未装配");
      setSavedValues(JSON.stringify(values));
      // 创建后回到列表，让新档案在真实查询里出现。
      navigate(profileId ? `/profiles/${result.id}` : "/profiles");
    } catch (error) {
      setSubmitError(error instanceof Error ? error.message : "保存失败，请稍后重试");
    } finally {
      setSubmitting(false);
    }
  };
  const leave = () => {
    if (!dirty || window.confirm("有未保存的更改，确定离开吗？"))
      navigate(profileId ? `/profiles/${profileId}` : "/profiles");
  };

  return (
    <form className="profile-form" onSubmit={submit} noValidate>
      <div className="form-heading">
        <div>
          <p className="eyebrow">PROFILE CONFIGURATION</p>
          <h1>{profileId ? "编辑档案" : "创建档案"}</h1>
          <p className="form-lede">
            把身份、指纹、代理和运行限制放在同一处；凭据只显示是否已设置。
          </p>
        </div>
        <span className={dirty ? "save-state save-state--dirty" : "save-state"}>
          {dirty ? "未保存" : "已保存"}
        </span>
      </div>
      {submitError && (
        <p className="form-error form-error--top" role="alert">
          保存失败：{submitError}
        </p>
      )}
      <fieldset className="form-section">
        <legend>基本身份</legend>
        <div className="form-grid form-grid--two">
          <label className="field field--wide">
            <span>名称</span>
            <input
              ref={nameRef}
              aria-label="名称"
              value={values.name}
              onChange={(event) => update("name", event.target.value)}
              aria-invalid={Boolean(errors.name)}
              aria-describedby={errors.name ? "name-error" : undefined}
            />
            {errors.name && (
              <small id="name-error" className="field-error">
                {errors.name}
              </small>
            )}
          </label>
          <label className="field">
            <span>Space / 分组</span>
            <input
              value={values.group_id ?? ""}
              onChange={(event) => update("group_id", event.target.value || null)}
              placeholder="例如 Growth"
            />
          </label>
          <label className="field field--wide">
            <span>标签</span>
            <input
              value={values.tags?.join(", ") ?? ""}
              onChange={(event) =>
                update(
                  "tags",
                  event.target.value
                    .split(",")
                    .map((tag) => tag.trim())
                    .filter(Boolean),
                )
              }
              placeholder="用逗号分隔"
            />
          </label>
          <label className="field field--wide">
            <span>备注</span>
            <textarea
              value={values.notes ?? ""}
              onChange={(event) => update("notes", event.target.value || null)}
              rows={3}
            />
          </label>
        </div>
      </fieldset>
      <fieldset className="form-section">
        <legend>指纹与宿主限制</legend>
        <div className="form-grid form-grid--three">
          <label className="field">
            <span>平台</span>
            <select
              value={values.fingerprint.platform}
              onChange={(event) =>
                updateFingerprint(
                  "platform",
                  event.target.value as ProfileInput["fingerprint"]["platform"],
                )
              }
            >
              <option value="macos">macOS</option>
              <option value="windows">Windows</option>
              <option value="linux">Linux</option>
            </select>
          </label>
          <label className="field">
            <span>平台版本</span>
            <input
              value={values.fingerprint.platform_version}
              onChange={(event) => updateFingerprint("platform_version", event.target.value)}
            />
          </label>
          <label className="field">
            <span>浏览器品牌</span>
            <select
              value={values.fingerprint.brand}
              onChange={(event) =>
                updateFingerprint(
                  "brand",
                  event.target.value as ProfileInput["fingerprint"]["brand"],
                )
              }
            >
              <option>Chrome</option>
              <option>Edge</option>
              <option>Opera</option>
              <option>Vivaldi</option>
            </select>
          </label>
          <label className="field">
            <span>核心数</span>
            <input
              type="number"
              min="1"
              value={values.fingerprint.hardware_concurrency}
              onChange={(event) =>
                updateFingerprint("hardware_concurrency", Number(event.target.value))
              }
            />
          </label>
          <label className="field">
            <span>语言</span>
            <input
              value={values.fingerprint.languages.join(", ")}
              onChange={(event) =>
                updateFingerprint(
                  "languages",
                  event.target.value
                    .split(",")
                    .map((item) => item.trim())
                    .filter(Boolean),
                )
              }
            />
          </label>
          <label className="field">
            <span>时区</span>
            <input
              value={values.fingerprint.timezone}
              onChange={(event) => updateFingerprint("timezone", event.target.value)}
            />
          </label>
          <label className="field">
            <span>窗口宽度</span>
            <input
              type="number"
              min="320"
              value={values.fingerprint.window.width}
              onChange={(event) => updateWindow("width", Number(event.target.value))}
            />
          </label>
          <label className="field">
            <span>窗口高度</span>
            <input
              type="number"
              min="240"
              value={values.fingerprint.window.height}
              onChange={(event) => updateWindow("height", Number(event.target.value))}
            />
          </label>
          <div className="field-note">
            <strong>Seed</strong>
            <span className="mono">{profile?.fingerprint.seed ?? "创建后由服务端生成"}</span>
            <small>只读；字体、插件面和屏幕字段受宿主约束。</small>
          </div>
        </div>
      </fieldset>
      <fieldset className="form-section">
        <legend>代理与运行</legend>
        <div className="form-grid form-grid--two">
          <label className="field">
            <span>代理 ID</span>
            <input
              value={values.proxy_id ?? ""}
              onChange={(event) => update("proxy_id", event.target.value || null)}
              placeholder="留空表示直连"
            />
            <small>凭据不会回显；仅显示已设置状态。</small>
          </label>
          <label className="field">
            <span>启动地址</span>
            <input
              type="url"
              value={values.start_url ?? ""}
              onChange={(event) => update("start_url", event.target.value)}
              placeholder="https://example.com"
            />
          </label>
          <label className="field">
            <span>CDP 自动化</span>
            <input
              type="checkbox"
              checked={values.automation_api}
              onChange={(event) => update("automation_api", event.target.checked)}
            />{" "}
            <small>启用后暴露自动化端口。</small>
          </label>
        </div>
        <div className="mode-picker">
          <span className="field-label">运行模式</span>
          {(Object.keys(modeCopy) as RuntimeMode[]).map((mode) => (
            <label
              key={mode}
              className={`mode-option${values.mode === mode ? " mode-option--selected" : ""}`}
            >
              <input
                type="radio"
                name="mode"
                value={mode}
                checked={values.mode === mode}
                onChange={() => update("mode", mode)}
              />
              <span>
                <strong>{modeCopy[mode].label}</strong>
                <small>{modeCopy[mode].description}</small>
              </span>
            </label>
          ))}
        </div>
        <p className="limit-callout">
          宿主绑定、字体与 plugins 限制会随环境变化；无头与伪有头持续显示降低保证。unsandboxed /
          unknown 也会在档案证据中保留。
        </p>
      </fieldset>
      <div className="form-actions">
        <button className="button button--quiet" type="button" onClick={leave}>
          取消
        </button>
        <button className="button button--primary" type="submit" disabled={submitting}>
          {submitting ? "保存中…" : profileId ? "保存档案" : "创建档案"}
        </button>
      </div>
    </form>
  );
}

function profileToInput(profile?: Profile): ProfileInput {
  if (!profile) return structuredClone(defaultProfileInput);
  return {
    name: profile.name,
    group_id: profile.group_id,
    tags: profile.tags,
    notes: profile.notes,
    fingerprint: {
      ...profile.fingerprint,
      window: { ...profile.fingerprint.window },
      languages: [...profile.fingerprint.languages],
    },
    proxy_id: profile.proxy_id,
    mode: profile.mode,
    automation_api: profile.automation_api ?? false,
    start_url: profile.start_url ?? "",
  };
}
function toPatch(values: ProfileInput): ProfilePatch {
  const { name, group_id, tags, notes, fingerprint, proxy_id, mode, automation_api, start_url } =
    values;
  return { name, group_id, tags, notes, fingerprint, proxy_id, mode, automation_api, start_url };
}

/**
 * 命令参数按 ADR-0008 用 camelCase 顶层键；`fingerprint` 内部保持契约的
 * snake_case 字段。这里显式映射，避免把 OpenAPI 的 snake_case body 直接当 IPC 参数。
 */
function toCreateArgs(values: ProfileInput) {
  return {
    name: values.name,
    fingerprint: values.fingerprint,
    mode: values.mode,
    startUrl: values.start_url || undefined,
    notes: values.notes ?? undefined,
    groupId: values.group_id || undefined,
    tags: values.tags ?? [],
    proxyId: values.proxy_id || undefined,
    // 契约 `ProfileInput.automation_api` 默认 false；不传就永远拿不到 CDP 端口，
    // 自动化 API 开关在 UI 上等于无效。
    automationApi: values.automation_api ?? false,
  };
}
