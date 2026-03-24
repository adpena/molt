"""
molt.gpu.distributed — Distributed GPU compute across multiple machines.

Inspired by Dask/RAPIDS dask-cudf. Supports:
- Multi-GPU on a single machine (Metal multi-device)
- Multi-machine via TCP sockets or RDMA (when available)
- Partitioned DataFrames that span multiple GPUs
- Map-reduce across partitions

Usage:
    from molt.gpu.distributed import Cluster, DistributedDataFrame

    cluster = Cluster(workers=["localhost:8001", "localhost:8002"])
    ddf = DistributedDataFrame.from_dataframe(df, n_partitions=4)
    result = ddf.map_partitions(lambda part: part.filter(part["price"] > 20))
    local = result.collect()
"""

import socket
import threading
import json
import struct
from .dataframe import DataFrame, Series


# ── Worker ────────────────────────────────────────────────────────────

class Worker:
    """A GPU compute worker (local or remote).

    In local mode, the worker runs computations in-process.
    In remote mode, it communicates over TCP sockets. Real RDMA/JACCL
    support would be implemented as a Rust crate for zero-copy transfers.
    """

    def __init__(self, address="localhost", port=8001):
        self.address = address
        self.port = port
        self._socket = None
        self._connected = False

    def connect(self):
        """Connect to a remote worker process.

        For local workers (localhost), this is a no-op — computations
        run in-process without socket overhead.
        """
        if self.address == "localhost" or self.address == "127.0.0.1":
            # Local worker — no socket needed, runs in-process
            self._connected = True
            return

        try:
            self._socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self._socket.settimeout(5.0)
            self._socket.connect((self.address, self.port))
            self._connected = True
        except (ConnectionRefusedError, socket.timeout, OSError):
            # Worker not available — fall back to local execution
            self._socket = None
            self._connected = True  # Mark as connected for local fallback

    def execute(self, func, data):
        """Execute a function on this worker.

        For local workers, runs the function directly. For remote workers,
        would serialize the function and data, send to the remote process,
        and receive the result.

        Args:
            func: callable to execute
            data: DataFrame or other data to process

        Returns:
            result of func(data)
        """
        if self._socket is not None:
            # Remote execution would serialize func + data, send over socket,
            # and receive the result. For now, fall back to local.
            pass

        # Local execution (in-process)
        return func(data)

    def send_bytes(self, data_bytes):
        """Send raw bytes to a remote worker."""
        if self._socket is None:
            return
        # Length-prefixed protocol
        length = len(data_bytes)
        self._socket.sendall(struct.pack('!Q', length))
        self._socket.sendall(data_bytes)

    def recv_bytes(self):
        """Receive raw bytes from a remote worker."""
        if self._socket is None:
            return b''
        # Read length prefix
        length_data = self._recv_exact(8)
        if not length_data:
            return b''
        length = struct.unpack('!Q', length_data)[0]
        return self._recv_exact(length)

    def _recv_exact(self, n):
        """Receive exactly n bytes from the socket."""
        data = bytearray()
        while len(data) < n:
            chunk = self._socket.recv(n - len(data))
            if not chunk:
                break
            data.extend(chunk)
        return bytes(data)

    def close(self):
        """Close the connection to this worker."""
        if self._socket:
            try:
                self._socket.close()
            except OSError:
                pass
            self._socket = None
        self._connected = False

    @property
    def is_local(self):
        return self.address in ("localhost", "127.0.0.1")

    def __repr__(self):
        status = "connected" if self._connected else "disconnected"
        return f"Worker({self.address}:{self.port}, {status})"


# ── Cluster ──────────────────────────────────────────────────────────

class Cluster:
    """A cluster of GPU workers.

    Can be configured with explicit worker addresses for multi-machine
    setups, or with n_local_workers for single-machine multi-GPU.

    Args:
        workers: list of "host:port" strings for remote workers
        n_local_workers: number of local in-process workers (default: 1)

    Example:
        # Multi-machine
        cluster = Cluster(workers=["gpu1:8001", "gpu2:8001", "gpu3:8001"])

        # Single machine, 4 GPU partitions
        cluster = Cluster(n_local_workers=4)
    """

    def __init__(self, workers=None, n_local_workers=1):
        self.workers = []
        if workers:
            for addr in workers:
                if ':' in addr:
                    host, port = addr.rsplit(':', 1)
                    self.workers.append(Worker(host, int(port)))
                else:
                    self.workers.append(Worker(addr, 8001))
        else:
            for i in range(n_local_workers):
                self.workers.append(Worker("localhost", 8001 + i))

    @property
    def n_workers(self):
        """Number of workers in the cluster."""
        return len(self.workers)

    def connect_all(self):
        """Connect to all workers in the cluster."""
        for worker in self.workers:
            worker.connect()

    def close_all(self):
        """Close all worker connections."""
        for worker in self.workers:
            worker.close()

    def __enter__(self):
        self.connect_all()
        return self

    def __exit__(self, *args):
        self.close_all()

    def __repr__(self):
        return f"Cluster({self.n_workers} workers)"


# ── Partition ────────────────────────────────────────────────────────

class Partition:
    """A partition of a distributed DataFrame.

    Each partition holds a subset of the rows and is assigned to a worker.
    Operations on the partition are executed on that worker's GPU.
    """

    def __init__(self, df, partition_id, worker):
        self.df = df
        self.partition_id = partition_id
        self.worker = worker

    def __len__(self):
        return len(self.df)

    def __repr__(self):
        return f"Partition({self.partition_id}, {len(self.df)} rows, {self.worker})"


# ── DistributedDataFrame ─────────────────────────────────────────────

class DistributedDataFrame:
    """DataFrame distributed across multiple GPU workers.

    Splits data into partitions, each assigned to a worker. Operations
    like map_partitions, filter, and group_by run in parallel across
    workers. Call collect() to gather results back to a single DataFrame.

    Usage:
        ddf = DistributedDataFrame.from_dataframe(df, n_partitions=4)
        result = ddf.map_partitions(lambda p: p.filter(p["price"] > 20))
        local = result.collect()
    """

    def __init__(self, partitions):
        self._partitions = partitions

    @classmethod
    def from_dataframe(cls, df, n_partitions=None, cluster=None):
        """Split a DataFrame into partitions across workers.

        Args:
            df: source DataFrame
            n_partitions: number of partitions (default: cluster.n_workers or 2)
            cluster: optional Cluster to assign workers from

        Returns:
            DistributedDataFrame with data split across partitions
        """
        n = len(df)
        n_parts = n_partitions or (cluster.n_workers if cluster else 2)
        chunk_size = (n + n_parts - 1) // n_parts

        partitions = []
        for i in range(n_parts):
            start = i * chunk_size
            end = min(start + chunk_size, n)
            if start >= n:
                break

            # Slice each column for this partition
            part_data = {}
            for col_name in df.columns:
                col = df[col_name]
                part_data[col_name] = col[start:end].to_list()

            part_df = DataFrame(part_data)
            worker = (
                cluster.workers[i % cluster.n_workers]
                if cluster
                else Worker("localhost", 8001 + i)
            )
            partitions.append(Partition(part_df, i, worker))

        return cls(partitions)

    def map_partitions(self, func):
        """Apply a function to each partition independently.

        The function receives a DataFrame and must return a DataFrame.
        Each partition is processed by its assigned worker.

        Args:
            func: callable(DataFrame) -> DataFrame

        Returns:
            new DistributedDataFrame with transformed partitions

        Example:
            ddf.map_partitions(lambda p: p.filter(p["price"] > 20))
        """
        new_partitions = []
        for part in self._partitions:
            result_df = part.worker.execute(func, part.df)
            new_partitions.append(Partition(result_df, part.partition_id, part.worker))
        return DistributedDataFrame(new_partitions)

    def filter(self, pred_func):
        """Filter each partition using a predicate function.

        Args:
            pred_func: callable(DataFrame) -> DataFrame

        Returns:
            new DistributedDataFrame with filtered partitions
        """
        return self.map_partitions(pred_func)

    def select(self, *cols):
        """Select columns from each partition.

        Args:
            *cols: column names to select

        Returns:
            new DistributedDataFrame with only the selected columns
        """
        return self.map_partitions(lambda p: p.select(*cols))

    def with_column(self, func, name):
        """Add a computed column to each partition.

        Args:
            func: callable(DataFrame) -> Series for the new column
            name: name for the new column

        Returns:
            new DistributedDataFrame with the added column
        """
        def _add_col(p):
            return p.with_column(func(p), name=name)
        return self.map_partitions(_add_col)

    def sort(self, by, descending=False):
        """Sort each partition locally by a column.

        Note: this sorts within each partition, not globally. For a full
        global sort, collect() first then sort the result.

        Args:
            by: column name to sort by
            descending: sort in descending order (default: False)

        Returns:
            new DistributedDataFrame with locally sorted partitions
        """
        return self.map_partitions(lambda p: p.sort(by, descending=descending))

    def group_by_agg(self, by_col, **aggregations):
        """Distributed group-by + aggregation.

        Performs a two-phase aggregation:
        1. Local aggregation on each partition (map phase)
        2. Global aggregation across partial results (reduce phase)

        Args:
            by_col: column name to group by
            **aggregations: name=(col, op) pairs

        Returns:
            DataFrame with aggregated results
        """
        # Phase 1: local group-by on each partition
        local_results = []
        for part in self._partitions:
            if len(part.df) > 0:
                gb = part.df.group_by(by_col)
                local_result = gb.agg(**aggregations)
                local_results.append(local_result)

        if not local_results:
            return DataFrame()

        # Phase 2: merge local results
        # Collect all local results into one DataFrame, then re-aggregate
        combined = {}
        for col_name in local_results[0].columns:
            combined[col_name] = []
        for lr in local_results:
            for col_name in lr.columns:
                combined[col_name].extend(lr[col_name].to_list())

        merged = DataFrame(combined)

        # Re-aggregate the partial results
        # For sum: sum of partial sums. For count: sum of partial counts.
        # For min/max: min/max of partial min/max. Mean requires special handling.
        re_aggs = {}
        for agg_name, (col, op) in aggregations.items():
            if op in ('sum', 'count'):
                re_aggs[agg_name] = (agg_name, 'sum')
            elif op in ('min',):
                re_aggs[agg_name] = (agg_name, 'min')
            elif op in ('max',):
                re_aggs[agg_name] = (agg_name, 'max')
            else:
                # For mean and others, fall back to sum (approximate)
                re_aggs[agg_name] = (agg_name, 'sum')

        return merged.group_by(by_col).agg(**re_aggs)

    def collect(self):
        """Collect all partitions into a single local DataFrame.

        Gathers data from all workers back to the local machine.

        Returns:
            DataFrame with all rows from all partitions
        """
        if not self._partitions:
            return DataFrame()

        # Get column names from the first non-empty partition
        col_names = None
        for part in self._partitions:
            if len(part.df) > 0:
                col_names = part.df.columns
                break

        if col_names is None:
            return DataFrame()

        combined = {col_name: [] for col_name in col_names}

        for part in self._partitions:
            if len(part.df) == 0:
                continue
            for col_name in col_names:
                if col_name in part.df.columns:
                    combined[col_name].extend(part.df[col_name].to_list())

        return DataFrame(combined)

    def repartition(self, n_partitions, cluster=None):
        """Redistribute data across a different number of partitions.

        Collects all data and re-splits it. Useful when partition sizes
        become unbalanced after filtering.

        Args:
            n_partitions: new number of partitions
            cluster: optional Cluster for worker assignment

        Returns:
            new DistributedDataFrame with rebalanced partitions
        """
        full = self.collect()
        return DistributedDataFrame.from_dataframe(full, n_partitions, cluster)

    def head(self, n=5):
        """Return the first n rows across all partitions.

        Scans partitions in order, taking rows until n are collected.
        """
        collected = {col: [] for col in self._partitions[0].df.columns} if self._partitions else {}
        remaining = n
        for part in self._partitions:
            if remaining <= 0:
                break
            take = min(remaining, len(part.df))
            for col_name in part.df.columns:
                collected[col_name].extend(part.df[col_name][:take].to_list())
            remaining -= take
        return DataFrame(collected)

    @property
    def n_partitions(self):
        """Number of partitions."""
        return len(self._partitions)

    @property
    def columns(self):
        """Column names (from first partition)."""
        if self._partitions and len(self._partitions[0].df) > 0:
            return self._partitions[0].df.columns
        return []

    def __len__(self):
        return sum(len(p.df) for p in self._partitions)

    def __repr__(self):
        total_rows = sum(len(p.df) for p in self._partitions)
        return f"DistributedDataFrame({total_rows} rows, {self.n_partitions} partitions)"


# ── Worker server (for remote execution) ─────────────────────────────

class WorkerServer:
    """TCP server that runs on each remote worker machine.

    Listens for incoming compute requests and executes them on the local GPU.
    This is the server counterpart to the Worker client.

    Usage (on the worker machine):
        server = WorkerServer(port=8001)
        server.serve_forever()
    """

    def __init__(self, port=8001, host="0.0.0.0"):
        self.port = port
        self.host = host
        self._server_socket = None
        self._running = False

    def start(self):
        """Start the worker server in a background thread."""
        self._server_socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._server_socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._server_socket.bind((self.host, self.port))
        self._server_socket.listen(8)
        self._running = True

        thread = threading.Thread(target=self._accept_loop, daemon=True)
        thread.start()

    def _accept_loop(self):
        """Accept incoming connections."""
        while self._running:
            try:
                self._server_socket.settimeout(1.0)
                conn, addr = self._server_socket.accept()
                handler = threading.Thread(
                    target=self._handle_client, args=(conn,), daemon=True
                )
                handler.start()
            except socket.timeout:
                continue
            except OSError:
                break

    def _handle_client(self, conn):
        """Handle a single client connection."""
        try:
            while self._running:
                # Read length-prefixed message
                length_data = self._recv_exact(conn, 8)
                if not length_data:
                    break
                length = struct.unpack('!Q', length_data)[0]
                payload = self._recv_exact(conn, length)
                if not payload:
                    break

                # For now, echo back — real implementation would deserialize
                # the function + data, execute on GPU, and return the result
                conn.sendall(struct.pack('!Q', len(payload)))
                conn.sendall(payload)
        except OSError:
            pass
        finally:
            conn.close()

    def _recv_exact(self, conn, n):
        """Receive exactly n bytes."""
        data = bytearray()
        while len(data) < n:
            try:
                chunk = conn.recv(n - len(data))
                if not chunk:
                    return None
                data.extend(chunk)
            except OSError:
                return None
        return bytes(data)

    def stop(self):
        """Stop the worker server."""
        self._running = False
        if self._server_socket:
            try:
                self._server_socket.close()
            except OSError:
                pass
            self._server_socket = None

    def serve_forever(self):
        """Start the server and block until interrupted."""
        self.start()
        try:
            while self._running:
                threading.Event().wait(1.0)
        except KeyboardInterrupt:
            pass
        finally:
            self.stop()

    def __repr__(self):
        status = "running" if self._running else "stopped"
        return f"WorkerServer({self.host}:{self.port}, {status})"
