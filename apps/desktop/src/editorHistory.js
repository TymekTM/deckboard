// editorHistory.js - Command-pattern undo/redo history for Pulpit editor.
// Depth 100, supports tile and board operations with ID remapping.

export function createEditorHistory(maxDepth = 100) {
  let undoStack = [];
  let redoStack = [];

  function canUndo() {
    return undoStack.length > 0;
  }

  function canRedo() {
    return redoStack.length > 0;
  }

  function clear() {
    undoStack = [];
    redoStack = [];
  }

  function push(entry) {
    if (!entry || typeof entry.undo !== "function" || typeof entry.redo !== "function") {
      throw new Error("History entry must implement undo and redo functions");
    }
    undoStack.push(entry);
    if (undoStack.length > maxDepth) {
      undoStack.shift();
    }
    redoStack = [];
  }

  function remapTileId(oldId, newId) {
    if (oldId === newId) return;
    for (const entry of [...undoStack, ...redoStack]) {
      if (typeof entry.remapTileId === "function") {
        entry.remapTileId(oldId, newId);
      }
    }
  }

  function remapBoardId(oldId, newId) {
    if (oldId === newId) return;
    for (const entry of [...undoStack, ...redoStack]) {
      if (typeof entry.remapBoardId === "function") {
        entry.remapBoardId(oldId, newId);
      }
    }
  }

  async function undo(api, hooks = {}) {
    if (!canUndo()) return false;
    const entry = undoStack.pop();
    try {
      await entry.undo(api, { remapTileId, remapBoardId, ...hooks });
      redoStack.push(entry);
      return true;
    } catch (err) {
      console.error("Undo failed:", err);
      throw err;
    }
  }

  async function redo(api, hooks = {}) {
    if (!canRedo()) return false;
    const entry = redoStack.pop();
    try {
      await entry.redo(api, { remapTileId, remapBoardId, ...hooks });
      undoStack.push(entry);
      return true;
    } catch (err) {
      console.error("Redo failed:", err);
      throw err;
    }
  }

  return {
    push,
    undo,
    redo,
    canUndo,
    canRedo,
    clear,
    remapTileId,
    remapBoardId,
    get undoCount() {
      return undoStack.length;
    },
    get redoCount() {
      return redoStack.length;
    },
  };
}

// Helpers to create standard history entries

export function tileCreateCommand({ tileSnapshot, boardId }) {
  let currentId = tileSnapshot.id;
  let currentBoardId = boardId;

  return {
    description: `Utwórz kafel: ${tileSnapshot.title || tileSnapshot.type}`,
    remapTileId(oldId, newId) {
      if (currentId === oldId) currentId = newId;
    },
    remapBoardId(oldId, newId) {
      if (currentBoardId === oldId) currentBoardId = newId;
    },
    async undo(api) {
      await api.deleteButton(currentId, currentBoardId);
    },
    async redo(api, { remapTileId }) {
      const newId = await api.createButton(
        currentBoardId,
        tileSnapshot.type || "key",
        tileSnapshot.mode || "button",
        tileSnapshot.x || 0,
        tileSnapshot.y || 0
      );
      if (newId !== currentId) {
        remapTileId(currentId, newId);
        currentId = newId;
      }
      await api.updateButton({
        ...tileSnapshot,
        id: currentId,
        board_id: currentBoardId,
      });
    },
  };
}

export function tileEditCommand({ prevSnapshot, nextSnapshot }) {
  let tileId = nextSnapshot.id;
  let prev = { ...prevSnapshot };
  let next = { ...nextSnapshot };

  return {
    description: `Edytuj kafel: ${next.title || next.type}`,
    remapTileId(oldId, newId) {
      if (tileId === oldId) {
        tileId = newId;
        prev.id = newId;
        next.id = newId;
      }
    },
    remapBoardId(oldId, newId) {
      if (prev.board_id === oldId) prev.board_id = newId;
      if (next.board_id === oldId) next.board_id = newId;
    },
    async undo(api) {
      await api.updateButton({ ...prev, id: tileId });
    },
    async redo(api) {
      await api.updateButton({ ...next, id: tileId });
    },
  };
}

export function tileDeleteCommand({ tileSnapshot }) {
  let currentId = tileSnapshot.id;
  let boardId = tileSnapshot.board_id;
  let snapshot = { ...tileSnapshot };

  return {
    description: `Usuń kafel: ${snapshot.title || snapshot.type}`,
    remapTileId(oldId, newId) {
      if (currentId === oldId) {
        currentId = newId;
        snapshot.id = newId;
      }
    },
    remapBoardId(oldId, newId) {
      if (boardId === oldId) {
        boardId = newId;
        snapshot.board_id = newId;
      }
    },
    async undo(api, { remapTileId }) {
      const newId = await api.createButton(
        boardId,
        snapshot.type || "key",
        snapshot.mode || "button",
        snapshot.x ?? 0,
        snapshot.y ?? 0
      );
      if (newId !== currentId) {
        remapTileId(currentId, newId);
        currentId = newId;
      }
      await api.updateButton({
        ...snapshot,
        id: currentId,
        board_id: boardId,
      });
    },
    async redo(api) {
      await api.deleteButton(currentId, boardId);
    },
  };
}

export function tileMoveCommand({ id, boardId, prevGeom, nextGeom }) {
  let currentId = id;
  let currentBoardId = boardId;

  return {
    description: "Przesuń / zmień rozmiar kafla",
    remapTileId(oldId, newId) {
      if (currentId === oldId) currentId = newId;
    },
    remapBoardId(oldId, newId) {
      if (currentBoardId === oldId) currentBoardId = newId;
    },
    async undo(api) {
      await api.moveButton(
        currentId,
        currentBoardId,
        prevGeom.x,
        prevGeom.y,
        prevGeom.w,
        prevGeom.h
      );
    },
    async redo(api) {
      await api.moveButton(
        currentId,
        currentBoardId,
        nextGeom.x,
        nextGeom.y,
        nextGeom.w,
        nextGeom.h
      );
    },
  };
}

export function tileBulkMoveCommand({ moves }) {
  // moves: [{ id, boardId, prevGeom: {x,y,w,h}, nextGeom: {x,y,w,h} }]
  let currentMoves = moves.map((m) => ({ ...m }));

  return {
    description: "Przesuń wiele kafli",
    remapTileId(oldId, newId) {
      for (const m of currentMoves) {
        if (m.id === oldId) m.id = newId;
      }
    },
    remapBoardId(oldId, newId) {
      for (const m of currentMoves) {
        if (m.boardId === oldId) m.boardId = newId;
      }
    },
    async undo(api) {
      for (const m of currentMoves) {
        await api.moveButton(
          m.id,
          m.boardId,
          m.prevGeom.x,
          m.prevGeom.y,
          m.prevGeom.w,
          m.prevGeom.h
        );
      }
    },
    async redo(api) {
      for (const m of currentMoves) {
        await api.moveButton(
          m.id,
          m.boardId,
          m.nextGeom.x,
          m.nextGeom.y,
          m.nextGeom.w,
          m.nextGeom.h
        );
      }
    },
  };
}

export function tileBulkDeleteCommand({ tiles }) {
  let currentTiles = tiles.map((t) => ({ snapshot: { ...t }, currentId: t.id }));

  return {
    description: `Usuń ${tiles.length} kafli`,
    remapTileId(oldId, newId) {
      for (const item of currentTiles) {
        if (item.currentId === oldId) {
          item.currentId = newId;
          item.snapshot.id = newId;
        }
      }
    },
    remapBoardId(oldId, newId) {
      for (const item of currentTiles) {
        if (item.snapshot.board_id === oldId) {
          item.snapshot.board_id = newId;
        }
      }
    },
    async undo(api, { remapTileId }) {
      for (const item of currentTiles) {
        const s = item.snapshot;
        const newId = await api.createButton(
          s.board_id,
          s.type || "key",
          s.mode || "button",
          s.x ?? 0,
          s.y ?? 0
        );
        if (newId !== item.currentId) {
          remapTileId(item.currentId, newId);
          item.currentId = newId;
        }
        await api.updateButton({
          ...s,
          id: item.currentId,
        });
      }
    },
    async redo(api) {
      for (const item of currentTiles) {
        await api.deleteButton(item.currentId, item.snapshot.board_id);
      }
    },
  };
}

export function boardCreateCommand({ boardId, name, background, width, height }) {
  let currentId = boardId;

  return {
    description: `Utwórz tablicę: ${name}`,
    remapBoardId(oldId, newId) {
      if (currentId === oldId) currentId = newId;
    },
    async undo(api) {
      await api.deleteBoard(currentId);
    },
    async redo(api, { remapBoardId }) {
      const newId = await api.createBoard(name, background, width, height);
      if (newId !== currentId) {
        remapBoardId(currentId, newId);
        currentId = newId;
      }
    },
  };
}

export function boardEditCommand({ prevSnapshot, nextSnapshot }) {
  let currentId = nextSnapshot.id;
  let prev = { ...prevSnapshot };
  let next = { ...nextSnapshot };

  return {
    description: `Edytuj tablicę: ${next.name}`,
    remapBoardId(oldId, newId) {
      if (currentId === oldId) {
        currentId = newId;
        prev.id = newId;
        next.id = newId;
      }
    },
    async undo(api) {
      await api.updateBoard({ ...prev, id: currentId });
    },
    async redo(api) {
      await api.updateBoard({ ...next, id: currentId });
    },
  };
}

export function boardDeleteCommand({ boardSnapshot }) {
  let currentId = boardSnapshot.id;
  let snapshot = { ...boardSnapshot, buttons: (boardSnapshot.buttons || []).map((b) => ({ ...b })) };

  return {
    description: `Usuń tablicę: ${snapshot.name}`,
    remapBoardId(oldId, newId) {
      if (currentId === oldId) currentId = newId;
    },
    async undo(api, { remapBoardId, remapTileId }) {
      const newId = await api.createBoard(
        snapshot.name,
        snapshot.background || "#437072",
        snapshot.width || 5,
        snapshot.height || 3
      );
      if (newId !== currentId) {
        remapBoardId(currentId, newId);
        currentId = newId;
      }
      for (const b of snapshot.buttons) {
        const newTileId = await api.createButton(
          currentId,
          b.type || "key",
          b.mode || "button",
          b.x ?? 0,
          b.y ?? 0
        );
        if (newTileId !== b.id) {
          remapTileId(b.id, newTileId);
          b.id = newTileId;
        }
        await api.updateButton({
          ...b,
          id: newTileId,
          board_id: currentId,
        });
      }
    },
    async redo(api) {
      await api.deleteBoard(currentId);
    },
  };
}

export function boardClearCommand({ boardId, tilesSnapshot }) {
  let currentBoardId = boardId;
  let snapshots = tilesSnapshot.map((b) => ({ ...b }));

  return {
    description: "Wyczyść tablicę",
    remapBoardId(oldId, newId) {
      if (currentBoardId === oldId) currentBoardId = newId;
    },
    remapTileId(oldId, newId) {
      for (const b of snapshots) {
        if (b.id === oldId) b.id = newId;
      }
    },
    async undo(api, { remapTileId }) {
      for (const b of snapshots) {
        const newTileId = await api.createButton(
          currentBoardId,
          b.type || "key",
          b.mode || "button",
          b.x ?? 0,
          b.y ?? 0
        );
        if (newTileId !== b.id) {
          remapTileId(b.id, newTileId);
          b.id = newTileId;
        }
        await api.updateButton({
          ...b,
          id: newTileId,
          board_id: currentBoardId,
        });
      }
    },
    async redo(api) {
      await api.clearBoard(currentBoardId);
    },
  };
}
