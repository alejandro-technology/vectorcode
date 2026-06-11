"""Data processing utilities."""

from typing import List, Dict, Any, Callable
from functools import reduce


def filter_items(items: List[Dict[str, Any]], predicate: Callable[[Dict[str, Any]], bool]) -> List[Dict[str, Any]]:
    """Filter items based on a predicate function."""
    return [item for item in items if predicate(item)]


def transform_items(items: List[Dict[str, Any]], transformer: Callable[[Dict[str, Any]], Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Apply a transformation function to each item."""
    return [transformer(item) for item in items]


def aggregate_by_key(items: List[Dict[str, Any]], key: str) -> Dict[Any, List[Dict[str, Any]]]:
    """Group items by a specific key."""
    result: Dict[Any, List[Dict[str, Any]]] = {}
    for item in items:
        k = item.get(key)
        if k not in result:
            result[k] = []
        result[k].append(item)
    return result


def compute_statistics(values: List[float]) -> Dict[str, float]:
    """Compute basic statistics for a list of values."""
    if not values:
        return {"mean": 0.0, "min": 0.0, "max": 0.0, "count": 0.0}
    
    return {
        "mean": sum(values) / len(values),
        "min": min(values),
        "max": max(values),
        "count": len(values),
        "sum": sum(values),
    }


class DataPipeline:
    """A pipeline for processing data through multiple stages."""
    
    def __init__(self):
        self._stages: List[Callable[[List[Dict[str, Any]]], List[Dict[str, Any]]]] = []
    
    def add_stage(self, stage: Callable[[List[Dict[str, Any]]], List[Dict[str, Any]]]) -> "DataPipeline":
        """Add a processing stage to the pipeline."""
        self._stages.append(stage)
        return self
    
    def execute(self, data: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Execute the pipeline on the input data."""
        result = data
        for stage in self._stages:
            result = stage(result)
        return result
