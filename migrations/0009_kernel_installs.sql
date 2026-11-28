-- 内核安装记录。
--
-- 没有这张表时，`verify_kernel` 只能拿刚算出的摘要跟它自己比对，恒真。有了
-- 记录，启动时比对的是"安装那一刻写下的摘要"与"现在磁盘上的字节"，任何替换、
-- 半截写入或路径劫持都会暴露。
CREATE TABLE kernel_installs (
  -- 内核版本，例如 148.0.7778.215
  version TEXT NOT NULL,
  -- 平台三元组，例如 macos-arm64；同一版本在不同平台是不同安装。
  platform TEXT NOT NULL,
  -- 解包后可执行文件的 SHA-256（不是下载归档的摘要）。
  executable_sha256 TEXT NOT NULL,
  -- 安装到时使用的可执行文件绝对路径，供诊断；不作为信任依据。
  executable_path TEXT NOT NULL,
  -- 来源资产的摘要（DMG/tar 等），用于追溯这份安装来自哪个发布物。
  source_asset_sha256 TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (version, platform)
);
