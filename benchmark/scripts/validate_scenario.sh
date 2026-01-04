#!/bin/bash
cd benchmark
export PYTHONPATH=$(pwd)/src
echo "Cleaning up..."
python3 -m smi_bench.agentbeats_run_scenario scenario_smi --kill
echo "Launching scenario..."
python3 -m smi_bench.agentbeats_run_scenario scenario_smi --launch-mode current > scenario.log 2>&1 &
SCENARIO_PID=$!
sleep 10
echo "Checking status..."
python3 -m smi_bench.agentbeats_run_scenario scenario_smi --status
echo "Log tail:"
tail -n 20 scenario.log
echo "Killing scenario manager..."
kill $SCENARIO_PID
sleep 2
python3 -m smi_bench.agentbeats_run_scenario scenario_smi --kill
