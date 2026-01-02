import ast
from dataclasses import dataclass
from typing import List, Any

@dataclass
class MoltValue:
    name: str
    type_hint: str = "Unknown"

@dataclass
class MoltOp:
    kind: str
    args: List[Any]
    result: MoltValue
    metadata: dict = None

class SimpleTIRGenerator(ast.NodeVisitor):
    def __init__(self):
        self.funcs_map = {"molt_main": {"params": [], "ops": []}}
        self.current_func_name = "molt_main"
        self.current_ops = self.funcs_map["molt_main"]["ops"]
        self.var_count = 0
        self.state_count = 0
        self.classes = {} 
        self.locals = {} # name -> MoltValue
        self.globals = {} # name -> MoltValue
        self.async_locals = {} # name -> offset

    def next_var(self) -> str:
        name = f"v{self.var_count}"
        self.var_count += 1
        return name
    
    def emit(self, op: MoltOp):
        self.current_ops.append(op)

    def start_function(self, name, params=None):
        if name not in self.funcs_map:
            self.funcs_map[name] = {"params": params or [], "ops": []}
        self.current_func_name = name
        self.current_ops = self.funcs_map[name]["ops"]
        self.locals = {} 
        self.async_locals = {}

    def resume_function(self, name):
        self.current_func_name = name
        self.current_ops = self.funcs_map[name]["ops"]

    def is_async(self):
        return self.current_func_name.endswith("_poll")

    def visit_Name(self, node: ast.Name):
        if isinstance(node.ctx, ast.Load):
            if self.is_async():
                if node.id in self.async_locals:
                    offset = self.async_locals[node.id]
                    res = MoltValue(self.next_var())
                    self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res))
                    return res
            return self.locals.get(node.id)
        return node.id

    def visit_BinOp(self, node: ast.BinOp):
        left = self.visit(node.left)
        right = self.visit(node.right)
        res = MoltValue(self.next_var())
        if isinstance(node.op, ast.Add):
            op_kind = "ADD"
        elif isinstance(node.op, ast.Sub):
            op_kind = "SUB"
        elif isinstance(node.op, ast.Mult):
            op_kind = "MUL"
        else:
            op_kind = "UNKNOWN"
        self.emit(MoltOp(kind=op_kind, args=[left, right], result=res))
        return res

    def visit_Constant(self, node: ast.Constant):
        if isinstance(node.value, str):
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.value], result=res))
            return res
        res = MoltValue(self.next_var(), type_hint=type(node.value).__name__)
        self.emit(MoltOp(kind="CONST", args=[node.value], result=res))
        return res

    def visit_ClassDef(self, node: ast.ClassDef):
        fields = {}
        offset = 0
        for item in node.body:
            if isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name):
                fields[item.target.id] = offset
                offset += 8 
        self.classes[node.name] = {"fields": fields, "size": offset}
        return None

    def visit_Call(self, node: ast.Call):
        if isinstance(node.func, ast.Attribute):
            # ...
            if isinstance(node.func.value, ast.Name) and node.func.value.id == "molt_json":
                if node.func.attr == "parse":
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="JSON_PARSE", args=[arg], result=res))
                    return res
            elif isinstance(node.func.value, ast.Name) and node.func.value.id == "asyncio":
                if node.func.attr == "run":
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="ASYNC_BLOCK_ON", args=[arg], result=res))
                    return res
                elif node.func.attr == "sleep":
                    res = MoltValue(self.next_var(), type_hint="Future")
                    self.emit(MoltOp(kind="CALL_ASYNC", args=["molt_async_sleep"], result=res))
                    return res

        if isinstance(node.func, ast.Name):
            func_id = node.func.id
            if func_id == "print":
                arg = self.visit(node.args[0])
                self.emit(MoltOp(kind="PRINT", args=[arg], result=MoltValue("none")))
                return None
            elif func_id == "molt_spawn":
                arg = self.visit(node.args[0])
                self.emit(MoltOp(kind="SPAWN", args=[arg], result=MoltValue("none")))
                return None
            elif func_id == "molt_chan_new":
                res = MoltValue(self.next_var(), type_hint="Channel")
                self.emit(MoltOp(kind="CHAN_NEW", args=[], result=res))
                return res
            elif func_id == "molt_chan_send":
                chan = self.visit(node.args[0])
                val = self.visit(node.args[1])
                self.state_count += 1
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CHAN_SEND_YIELD", args=[chan, val, self.state_count], result=res))
                return res
            elif func_id == "molt_chan_recv":
                chan = self.visit(node.args[0])
                self.state_count += 1
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CHAN_RECV_YIELD", args=[chan, self.state_count], result=res))
                return res
            elif func_id in self.classes:
                res = MoltValue(self.next_var(), type_hint=func_id)
                self.emit(MoltOp(kind="ALLOC", args=[func_id], result=res))
                return res
            
            # Check locals then globals
            target_info = self.locals.get(func_id) or self.globals.get(func_id)
            if target_info and str(target_info.type_hint).startswith("AsyncFunc:"):
                parts = target_info.type_hint.split(":")
                poll_func = parts[1]
                closure_size = int(parts[2])
                args = [self.visit(arg) for arg in node.args]
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(MoltOp(kind="ALLOC_FUTURE", args=[poll_func, closure_size] + args, result=res))
                return res

            if target_info and str(target_info.type_hint).startswith("Func:"):
                target_name = target_info.type_hint.split(":")[1]
                args = [self.visit(arg) for arg in node.args]
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CALL", args=[target_name] + args, result=res))
                return res

            res = MoltValue(self.next_var(), type_hint="Unknown")
            self.emit(MoltOp(kind="CALL_DUMMY", args=[func_id], result=res))
            return res
        return None

    def visit_Attribute(self, node: ast.Attribute):
        obj = self.visit(node.value)
        if obj is None:
            obj = MoltValue("unknown_obj", type_hint="Unknown")
        res = MoltValue(self.next_var())
        class_name = list(self.classes.keys())[-1] if self.classes else "None"
        self.emit(MoltOp(kind="GUARDED_GETATTR", args=[obj, node.attr, class_name], result=res))
        return res

    def visit_Assign(self, node: ast.Assign):
        value_node = self.visit(node.value)
        for target in node.targets:
            if isinstance(target, ast.Attribute):
                obj = self.visit(target.value)
                self.emit(MoltOp(kind="SETATTR", args=[obj, target.attr, value_node], result=MoltValue("none")))
            elif isinstance(target, ast.Name):
                if self.is_async():
                    if target.id not in self.async_locals:
                        self.async_locals[target.id] = len(self.async_locals) * 8
                    offset = self.async_locals[target.id]
                    self.emit(MoltOp(kind="STORE_CLOSURE", args=["self", offset, value_node], result=MoltValue("none")))
                else:
                    self.locals[target.id] = value_node
        return None

    def visit_Compare(self, node: ast.Compare):
        left = self.visit(node.left)
        right = self.visit(node.comparators[0])
        res = MoltValue(self.next_var())
        op_kind = "LT" if isinstance(node.ops[0], ast.Lt) else "UNKNOWN"
        self.emit(MoltOp(kind=op_kind, args=[left, right], result=res))
        return res

    def visit_If(self, node: ast.If):
        cond = self.visit(node.test)
        self.emit(MoltOp(kind="IF_RETURN", args=[cond], result=MoltValue("none")))
        for item in node.body:
            self.visit(item)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        for item in node.orelse:
            self.visit(item)
        return None

    def visit_Return(self, node: ast.Return):
        val = self.visit(node.value) if node.value else None
        if val is None:
            val = MoltValue(self.next_var())
            self.emit(MoltOp(kind="CONST", args=[0], result=val))
        self.emit(MoltOp(kind="ret", args=[val], result=MoltValue("none")))
        return None

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef):
        poll_func_name = f"{node.name}_poll"
        prev_func = self.current_func_name
        prev_async_locals = self.async_locals
        
        # Add to globals to support calls from other scopes
        self.globals[node.name] = MoltValue(node.name, type_hint=f"AsyncFunc:{poll_func_name}:0") # Placeholder size
        
        self.start_function(poll_func_name, params=["self"])
        for i, arg in enumerate(node.args.args):
            self.async_locals[arg.arg] = i * 8
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        for item in node.body:
            self.visit(item)
        res = MoltValue(self.next_var())
        self.emit(MoltOp(kind="CONST", args=[0], result=res)) 
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        closure_size = len(self.async_locals) * 8
        self.resume_function(prev_func)
        self.async_locals = prev_async_locals
        # Update closure size
        self.globals[node.name] = MoltValue(node.name, type_hint=f"AsyncFunc:{poll_func_name}:{closure_size}")
        return None

    def visit_FunctionDef(self, node: ast.FunctionDef):
        func_name = node.name
        prev_func = self.current_func_name
        params = [arg.arg for arg in node.args.args]
        
        self.globals[func_name] = MoltValue(func_name, type_hint=f"Func:{func_name}")
        
        self.start_function(func_name, params=params)
        for arg in node.args.args:
            self.locals[arg.arg] = MoltValue(arg.arg, type_hint="int") 
        for item in node.body:
            self.visit(item)
        if not (self.current_ops and self.current_ops[-1].kind == "ret"):
            res = MoltValue(self.next_var())
            self.emit(MoltOp(kind="CONST", args=[0], result=res)) 
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self.resume_function(prev_func)
        return None

    def visit_Await(self, node: ast.Await):
        coro = self.visit(node.value)
        self.state_count += 1
        res = MoltValue(self.next_var())
        self.emit(MoltOp(kind="STATE_TRANSITION", args=[coro, self.state_count], result=res))
        return res

    def map_ops_to_json(self, ops):
        json_ops = []
        for op in ops:
            if op.kind == "CONST":
                json_ops.append({"kind": "const", "value": op.args[0], "out": op.result.name})
            elif op.kind == "CONST_STR":
                json_ops.append({"kind": "const_str", "s_value": op.args[0], "out": op.result.name})
            elif op.kind == "ADD":
                json_ops.append({"kind": "add", "args": [arg.name for arg in op.args], "out": op.result.name})
            elif op.kind == "SUB":
                json_ops.append({"kind": "sub", "args": [arg.name for arg in op.args], "out": op.result.name})
            elif op.kind == "MUL":
                json_ops.append({"kind": "mul", "args": [arg.name for arg in op.args], "out": op.result.name})
            elif op.kind == "LT":
                json_ops.append({"kind": "lt", "args": [arg.name for arg in op.args], "out": op.result.name})
            elif op.kind == "IF_RETURN":
                json_ops.append({"kind": "if_return", "args": [op.args[0].name]})
            elif op.kind == "END_IF":
                json_ops.append({"kind": "end_if"})
            elif op.kind == "CALL":
                target = op.args[0]
                json_ops.append({"kind": "call", "s_value": target, "args": [arg.name for arg in op.args[1:]], "out": op.result.name})
            elif op.kind == "PRINT":
                json_ops.append({"kind": "print", "args": [arg.name if hasattr(arg, 'name') else str(arg) for arg in op.args]})
            elif op.kind == "ALLOC":
                json_ops.append({"kind": "alloc", "value": self.classes[op.args[0]]["size"], "out": op.result.name})
            elif op.kind == "SETATTR":
                obj, attr, val = op.args
                offset = self.classes[list(self.classes.keys())[-1]]["fields"][attr] + 24
                json_ops.append({"kind": "store", "args": [obj.name, val.name], "value": offset})
            elif op.kind == "GETATTR":
                obj, attr = op.args
                offset = self.classes[list(self.classes.keys())[-1]]["fields"][attr] + 24
                json_ops.append({"kind": "load", "args": [obj.name], "value": offset, "out": op.result.name})
            elif op.kind == "GUARDED_GETATTR":
                obj, attr, expected_class = op.args
                offset = self.classes[expected_class]["fields"][attr] + 24
                json_ops.append({"kind": "guarded_load", "args": [obj.name], "s_value": attr, "value": offset, "out": op.result.name, "metadata": {"expected_type_id": 100}})
            elif op.kind == "JSON_PARSE":
                json_ops.append({"kind": "json_parse", "args": [arg.name if hasattr(arg, 'name') else str(arg) for arg in op.args], "out": op.result.name})
            elif op.kind == "ASYNC_BLOCK_ON":
                json_ops.append({"kind": "block_on", "args": [arg.name if hasattr(arg, 'name') else str(arg) for arg in op.args], "out": op.result.name})
            elif op.kind == "CALL_DUMMY":
                json_ops.append({"kind": "const", "value": 0, "out": op.result.name})
            elif op.kind == "ret":
                json_ops.append({"kind": "ret", "var": op.args[0].name})
            elif op.kind == "ALLOC_FUTURE":
                poll_func = op.args[0]
                size = op.args[1]
                args = op.args[2:]
                json_ops.append({
                    "kind": "alloc_future",
                    "s_value": poll_func,
                    "value": size,
                    "args": [arg.name for arg in args],
                    "out": op.result.name
                })
            elif op.kind == "STATE_SWITCH":
                json_ops.append({"kind": "state_switch"})
            elif op.kind == "SPAWN":
            
                json_ops.append({"kind": "spawn", "args": [op.args[0].name]})
            elif op.kind == "CHAN_NEW":
                json_ops.append({"kind": "chan_new", "out": op.result.name})
            elif op.kind == "CHAN_SEND_YIELD":
                chan, val, next_state = op.args
                json_ops.append({"kind": "chan_send_yield", "args": [chan.name, val.name], "value": next_state, "out": op.result.name})
            elif op.kind == "CHAN_RECV_YIELD":
                chan, next_state = op.args
                json_ops.append({"kind": "chan_recv_yield", "args": [chan.name], "value": next_state, "out": op.result.name})
            elif op.kind == "CALL_ASYNC":
                json_ops.append({"kind": "call_async", "s_value": op.args[0], "out": op.result.name})
            elif op.kind == "LOAD_CLOSURE":
                self_ptr, offset = op.args
                json_ops.append({"kind": "load", "args": [self_ptr], "value": offset, "out": op.result.name})
            elif op.kind == "STORE_CLOSURE":
                self_ptr, offset, val = op.args
                json_ops.append({"kind": "store", "args": [self_ptr, val.name], "value": offset})
        
        if ops and ops[-1].kind != "ret":
             json_ops.append({ "kind": "ret_void" })
        return json_ops

    def to_json(self):
        funcs_json = []
        for name, data in self.funcs_map.items():
            funcs_json.append({
                "name": name,
                "params": data["params"],
                "ops": self.map_ops_to_json(data["ops"])
            })
        return {"functions": funcs_json}

    

def compile_to_tir(source: str):
    tree = ast.parse(source)
    gen = SimpleTIRGenerator()
    gen.visit(tree)
    return gen.to_json()
