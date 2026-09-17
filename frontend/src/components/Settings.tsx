import React, { useState, useEffect } from 'react';
import { FiX, FiPlus, FiTrash2, FiFolder, FiRotateCcw, FiMonitor, FiMinimize2, FiVolume2, FiVolumeX } from 'react-icons/fi';
import { useConfigStore } from '../stores/configStore';
import { Config, DEFAULT_CONFIG, APP_VERSION } from '../types';
import { invoke } from '../services/bridge';

interface SettingsProps {
  onClose: () => void;
}

/** Default data directory under user home */
const getDefaultDataDir = (): string => {
  // Use backend-injected defaultDataDir (matches backend GetAppDataDir)
  return (typeof window !== 'undefined' && (window as any).__tauricpp__?.defaultDataDir) || '';
};

export default function Settings({ onClose }: SettingsProps) {
  const config = useConfigStore((s) => s.config);
  const saveConfig = useConfigStore((s) => s.saveConfig);
  const [localConfig, setLocalConfig] = useState<Config>({ ...config });
  const [newSegment, setNewSegment] = useState('');

  useEffect(() => {
    setLocalConfig({ ...config });
  }, [config]);

  const handleSave = async () => {
    await saveConfig(localConfig);
    onClose();
  };

  const handleAddSegment = () => {
    if (newSegment.trim() && !localConfig.segments.includes(newSegment.trim())) {
      setLocalConfig({
        ...localConfig,
        segments: [...localConfig.segments, newSegment.trim()],
      });
      setNewSegment('');
    }
  };

  const handleRemoveSegment = (index: number) => {
    setLocalConfig({
      ...localConfig,
      segments: localConfig.segments.filter((_, i) => i !== index),
    });
  };

  // Handle git-link clicks: open in default browser via TauriCPP
  const handleGitLinkClick = (e: React.MouseEvent) => {
    const gitLink = (e.target as HTMLElement).closest('.git-link');
    if (gitLink) {
      e.preventDefault();
      const url = gitLink.getAttribute('href');
      if (url) {
        invoke('shell_open', { url });
      }
    }
  };

  const handlePickFolder = async () => {
    try {
      const initialDir = localConfig.dataDir || getDefaultDataDir();
      const result = await invoke<{ success?: boolean; folder?: string }>('dialog.pick_folder', {
        title: '选择聊天记录目录',
        initial_dir: initialDir,
      });
      console.log('[Settings] dialog.pick_folder result:', JSON.stringify(result));
      if (result.success && result.folder) {
        setLocalConfig({ ...localConfig, dataDir: result.folder });
      }
    } catch (e) {
      console.error('Failed to pick folder:', e);
    }
  };

  const handleResetDir = () => {
    setLocalConfig({ ...localConfig, dataDir: '' });
  };

  const displayDataDir = localConfig.dataDir || getDefaultDataDir() || '~/.ipmsgpro';

  return (
    <div className="flex-1 flex flex-col bg-white">
      {/* Header */}
      <div className="h-14 border-b border-gray-200 flex items-center justify-between px-4 shrink-0">
        <h2 className="text-base font-medium text-gray-800">设置</h2>
        <button className="p-1 text-gray-400 hover:text-gray-600" onClick={onClose}>
          <FiX size={20} />
        </button>
      </div>

      {/* Settings content */}
      <div className="flex-1 overflow-y-auto p-6">
        <div className="max-w-lg space-y-6">
          {/* User info */}
          <Section title="用户信息">
            <div className="grid grid-cols-2 gap-3">
              <Field label="昵称">
                <input
                  type="text"
                  value={localConfig.nickname}
                  onChange={(e) => setLocalConfig({ ...localConfig, nickname: e.target.value })}
                  className="input-field"
                  placeholder="输入昵称"
                />
              </Field>
              <Field label="分组">
                <input
                  type="text"
                  value={localConfig.group}
                  onChange={(e) => setLocalConfig({ ...localConfig, group: e.target.value })}
                  className="input-field"
                  placeholder="输入分组"
                />
              </Field>
            </div>
          </Section>

          {/* Network */}
          <Section title="网络设置">
            <Field label="端口号">
              <input
                type="number"
                value={localConfig.port}
                onChange={(e) => setLocalConfig({ ...localConfig, port: parseInt(e.target.value) || 2425 })}
                className="input-field"
                min={1}
                max={65535}
              />
            </Field>

            <Field label="自动发现用户">
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={localConfig.autoDiscovery}
                  onChange={(e) => setLocalConfig({ ...localConfig, autoDiscovery: e.target.checked })}
                  className="w-4 h-4 text-primary-500 rounded"
                />
                <span className="text-sm text-gray-600">启动时自动搜索局域网用户</span>
              </label>
            </Field>
          </Section>

          {/* Segments */}
          <Section title="网段配置">
            <div className="space-y-2">
              {localConfig.segments.map((seg, i) => (
                <div key={i} className="flex items-center gap-2">
                  <span className="flex-1 text-sm bg-gray-50 px-3 py-1.5 rounded border border-gray-200">
                    {seg}
                  </span>
                  <button
                    className="p-1 text-red-400 hover:text-red-600"
                    onClick={() => handleRemoveSegment(i)}
                  >
                    <FiTrash2 size={14} />
                  </button>
                </div>
              ))}
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  value={newSegment}
                  onChange={(e) => setNewSegment(e.target.value)}
                  className="input-field flex-1"
                  placeholder="输入广播地址，如 192.168.1.255"
                  onKeyDown={(e) => e.key === 'Enter' && handleAddSegment()}
                />
                <button
                  className="p-1.5 text-primary-500 hover:text-primary-600"
                  onClick={handleAddSegment}
                >
                  <FiPlus size={16} />
                </button>
              </div>
            </div>
          </Section>

          {/* Data directory */}
          <Section title="数据目录">
            <Field label="聊天记录存储目录">
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  value={localConfig.dataDir || displayDataDir}
                  onChange={(e) => setLocalConfig({ ...localConfig, dataDir: e.target.value })}
                  className="input-field flex-1"
                  placeholder={`默认: ${getDefaultDataDir() || '~/.ipmsgpro'}`}
                />
                <button
                  className="p-1.5 text-gray-400 hover:text-primary-500 border border-gray-200 rounded hover:border-primary-300"
                  onClick={handlePickFolder}
                  title="浏览选择目录"
                >
                  <FiFolder size={16} />
                </button>
                <button
                  className="p-1.5 text-gray-400 hover:text-primary-500 border border-gray-200 rounded hover:border-primary-300"
                  onClick={handleResetDir}
                  title="恢复默认"
                >
                  <FiRotateCcw size={16} />
                </button>
              </div>
              {localConfig.dataDir ? (
                <p className="text-xs text-gray-400 mt-1 break-all">当前: {localConfig.dataDir}</p>
              ) : (
                <p className="text-xs text-gray-400 mt-1 break-all">默认: {displayDataDir}</p>
              )}
            </Field>
          </Section>

          {/* Notification sound */}
          <Section title="消息提示">
            <Field label="新消息提示音">
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={localConfig.notificationSound}
                  onChange={(e) => setLocalConfig({ ...localConfig, notificationSound: e.target.checked })}
                  className="w-4 h-4 text-primary-500 rounded"
                />
                <span className="text-sm text-gray-600">
                  {localConfig.notificationSound ? <FiVolume2 size={14} className="inline mr-1" /> : <FiVolumeX size={14} className="inline mr-1" />}
                  收到新消息时播放提示音
                </span>
              </label>
            </Field>
          </Section>

          {/* Window behavior */}
          <Section title="窗口行为">
            <Field label="关闭/最小化行为">
              <div className="flex gap-3">
                <button
                  className={`flex items-center gap-2 px-3 py-1.5 text-sm rounded border transition-colors ${
                    localConfig.minimizeBehavior === 'taskbar'
                      ? 'border-primary-500 bg-primary-50 text-primary-600'
                      : 'border-gray-200 text-gray-500 hover:border-primary-300'
                  }`}
                  onClick={() => setLocalConfig({ ...localConfig, minimizeBehavior: 'taskbar' })}
                >
                  <FiMinimize2 size={14} />
                  最小化到任务栏
                </button>
                <button
                  className={`flex items-center gap-2 px-3 py-1.5 text-sm rounded border transition-colors ${
                    localConfig.minimizeBehavior === 'tray'
                      ? 'border-primary-500 bg-primary-50 text-primary-600'
                      : 'border-gray-200 text-gray-500 hover:border-primary-300'
                  }`}
                  onClick={() => setLocalConfig({ ...localConfig, minimizeBehavior: 'tray' })}
                >
                  <FiMonitor size={14} />
                  最小化到系统托盘
                </button>
              </div>
              <p className="text-xs text-gray-400 mt-1">
                {localConfig.minimizeBehavior === 'tray'
                  ? '关闭窗口时隐藏到系统托盘，右键托盘图标可退出'
                  : '关闭窗口时退出程序'}
              </p>
            </Field>
          </Section>

          {/* Version info */}
          <Section title="关于">
            <div className="text-sm text-gray-500">
              倍信 (IPMsg Pro) v{APP_VERSION}
            </div>
            <div className="flex items-center gap-4 mt-2" onClick={handleGitLinkClick}>
              <a href="https://github.com/Emsoro/IPMsgPro" className="git-link" title="GitHub">
                <svg viewBox="0 0 16 16" width="16" height="16">
                  <path fill="currentColor" d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/>
                </svg>
              </a>
              <a href="https://gitee.com/masonwu21/ipmsg-pro" className="git-link" title="Gitee">
                <svg viewBox="0 0 1024 1024" width="16" height="16">
                  <path fill="currentColor" d="M512 1024q-104 0-199-40-92-39-163-110T40 711Q0 616 0 512t40-199Q79 221 150 150T313 40q95-40 199-40t199 40q92 39 163 110t110 163q40 95 40 199t-40 199q-39 92-110 163T711 984q-95 40-199 40z m259-569H480q-10 0-17.5 7.5T455 480v64q0 10 7.5 17.5T480 569h177q11 0 18.5 7.5T683 594v13q0 31-22.5 53.5T607 683H367q-11 0-18.5-7.5T341 657V417q0-31 22.5-53.5T417 341h354q11 0 18-7t7-18v-63q0-11-7-18t-18-7H417q-38 0-72.5 14T283 283q-27 27-41 61.5T228 417v354q0 11 7 18t18 7h373q46 0 85.5-22.5t62-62Q796 672 796 626V480q0-10-7-17.5t-18-7.5z"/>
                </svg>
              </a>
            </div>
            <p className="text-xs text-gray-400 mt-1">
              兼容飞秋和IPMsg v3.0协议的局域网即时通讯
            </p>
          </Section>
        </div>
      </div>

      {/* Footer */}
      <div className="border-t border-gray-200 p-4 flex justify-end gap-3 shrink-0">
        <button
          className="px-4 py-2 text-sm text-gray-600 bg-gray-100 rounded hover:bg-gray-200 transition-colors"
          onClick={onClose}
        >
          取消
        </button>
        <button
          className="px-4 py-2 text-sm text-white bg-primary-500 rounded hover:bg-primary-600 transition-colors"
          onClick={handleSave}
        >
          保存
        </button>
      </div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div>
      <h3 className="text-sm font-medium text-gray-700 mb-3">{title}</h3>
      <div className="space-y-3">{children}</div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <label className="block text-xs text-gray-500 mb-1">{label}</label>
      {children}
    </div>
  );
}
