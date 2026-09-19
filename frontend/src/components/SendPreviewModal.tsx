import React from 'react';
import { FiFile, FiX } from 'react-icons/fi';
import { formatFileSize } from '../utils/format';

interface SendPreviewModalProps {
  fileName: string;
  fileSize: number;
  onConfirm: () => void;
  onCancel: () => void;
}

/** Confirmation shown before a file picked from disk is sent. */
export default function SendPreviewModal({ fileName, fileSize, onConfirm, onCancel }: SendPreviewModalProps) {
  return (
    <div className="fixed inset-0 bg-black/40 z-50 flex items-center justify-center"
      onClick={onCancel}>
      <div className="bg-white rounded-lg shadow-xl w-[400px] max-h-[80vh] flex flex-col"
        onClick={e => e.stopPropagation()}>
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b">
          <h3 className="text-sm font-medium text-gray-800">发送文件</h3>
          <button className="p-1 text-gray-400 hover:text-gray-600" onClick={onCancel}>
            <FiX size={16} />
          </button>
        </div>

        {/* Preview */}
        <div className="px-4 py-3 flex-1 overflow-auto">
          <div className="flex items-center gap-3 p-3 bg-gray-50 rounded-lg">
            <FiFile size={28} className="text-gray-400 shrink-0" />
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium text-gray-800 truncate">{fileName}</p>
              <p className="text-xs text-gray-400">{formatFileSize(fileSize)}</p>
            </div>
          </div>
          <p className="text-xs text-gray-400 mt-2">文件将通过 TCP 传输发送给对方</p>
        </div>

        {/* Actions */}
        <div className="flex justify-end gap-2 px-4 py-3 border-t">
          <button
            className="px-4 py-1.5 text-sm text-gray-600 bg-gray-100 rounded hover:bg-gray-200 transition-colors"
            onClick={onCancel}
          >
            取消
          </button>
          <button
            className="px-4 py-1.5 text-sm text-white bg-primary-500 rounded hover:bg-primary-600 transition-colors"
            onClick={onConfirm}
          >
            发送
          </button>
        </div>
      </div>
    </div>
  );
}
