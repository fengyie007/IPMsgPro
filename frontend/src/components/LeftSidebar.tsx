import React from 'react';
import { FiMessageSquare, FiUsers, FiSettings } from 'react-icons/fi';
import { useConfigStore } from '../stores/configStore';
import { APP_VERSION } from '../types';

export type ViewMode = 'chat' | 'contacts' | 'settings';

interface LeftSidebarProps {
  viewMode: ViewMode;
  onViewChange: (mode: ViewMode) => void;
}

export default function LeftSidebar({ viewMode, onViewChange }: LeftSidebarProps) {

  return (
    <div className="w-[60px] bg-[#2C2C2C] flex flex-col items-center py-4 shrink-0">
      {/* Logo */}
      <div className="w-10 h-10 rounded-lg bg-primary-500 flex items-center justify-center mb-8">
        <span className="text-white font-bold text-lg">P</span>
      </div>

      {/* Navigation buttons */}
      <div className="flex flex-col items-center gap-4 flex-1">
        <NavButton
          icon={<FiMessageSquare size={22} />}
          active={viewMode === 'chat'}
          onClick={() => onViewChange('chat')}
          title="对话"
        />
        <NavButton
          icon={<FiUsers size={22} />}
          active={viewMode === 'contacts'}
          onClick={() => onViewChange('contacts')}
          title="通讯录"
        />
        <NavButton
          icon={<FiSettings size={22} />}
          active={viewMode === 'settings'}
          onClick={() => onViewChange('settings')}
          title="设置"
        />
      </div>

      {/* Version at bottom */}
      <div className="mt-auto pt-2 text-[11px] leading-none text-gray-500 select-none" title="迅秋 (SpeedIPMsg)">
        v{APP_VERSION}
      </div>
    </div>
  );
}

function NavButton({ icon, active, onClick, title }: {
  icon: React.ReactNode;
  active: boolean;
  onClick: () => void;
  title: string;
}) {
  return (
    <button
      className={`w-10 h-10 rounded-lg flex items-center justify-center transition-colors relative
        ${active ? 'bg-[#3C3C3C] text-primary-400' : 'text-gray-400 hover:bg-[#3C3C3C] hover:text-gray-200'}`}
      onClick={onClick}
      title={title}
    >
      {active && (
        <div className="absolute left-0 top-1/2 -translate-y-1/2 w-[3px] h-5 bg-primary-500 rounded-r" />
      )}
      {icon}
    </button>
  );
}
