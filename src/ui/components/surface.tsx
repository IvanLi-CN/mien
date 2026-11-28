import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";

/** surface 占位：标题 + 归属 Task。 */
export function Surface({
  titleKey,
  task,
  hintKey,
  children,
}: {
  titleKey: string;
  hintKey?: string;
  task: string;
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <section className="flex flex-col gap-3">
      <header className="flex items-baseline gap-3">
        <h1 className="font-medium text-lg">{t(titleKey)}</h1>
        <span className="text-muted text-xs">
          {t("hints.skeleton")} · {task}
        </span>
      </header>
      {hintKey !== undefined && <p className="text-muted text-sm">{t(hintKey)}</p>}
      {children}
    </section>
  );
}
