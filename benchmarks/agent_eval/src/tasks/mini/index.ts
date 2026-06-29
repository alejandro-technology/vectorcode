import { Task } from '../../types.js';
import { taskMiniErrorDerive } from './task-error-derive.js';
import { taskMiniMergeTrace } from './task-merge-trace.js';
import { taskMiniSigningFlow } from './task-signing-flow.js';
import { taskMiniCrossRepo } from './task-cross-repo.js';

export const miniTasks: Task[] = [
  taskMiniErrorDerive,
  taskMiniMergeTrace,
  taskMiniSigningFlow,
  taskMiniCrossRepo,
];
