import { Task } from '../../types.js';
import { taskMockErrorLookup } from './task-error-lookup.js';
import { taskMockCrossLang } from './task-cross-lang.js';

export const mockMiniTasks: Task[] = [
  taskMockErrorLookup,
  taskMockCrossLang,
];
