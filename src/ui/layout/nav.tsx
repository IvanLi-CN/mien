import { useTranslation } from "react-i18next";
import { NavLink } from "react-router";

const links = [
  { to: "/profiles", ns: "profiles" },
  { to: "/proxies", ns: "proxies" },
  { to: "/kernels", ns: "kernels" },
  { to: "/activity", ns: "activity" },
  { to: "/settings", ns: "settings" },
] as const;

/** 一级导航；路由表见 `docs/UI-SURFACES.md` §1。 */
export function Nav() {
  const { t } = useTranslation();
  return (
    <nav aria-label={t("appName")} className="flex gap-1">
      {links.map((link) => (
        <NavLink
          key={link.to}
          to={link.to}
          className={({ isActive }) =>
            [
              "rounded-md px-2.5 py-1 text-sm transition-colors",
              isActive ? "bg-surface text-fg" : "text-muted hover:text-fg",
            ].join(" ")
          }
        >
          {t(link.ns)}
        </NavLink>
      ))}
    </nav>
  );
}
