import argparse
import os
import time
import math
import operator
import pyrem.host
# import pyrem.task
# from pyrem.host import RemoteHost
# import pyrem
import sys
import glob

import numpy as np
import matplotlib.pyplot as plt
import matplotlib.ticker as tick
from math import factorial, exp
from datetime import datetime
import signal
import atexit
from os.path import exists
import toml

import subprocess
from cycler import cycler

import datetime
import pty


from test_config import *

final_result = ''


def kill_procs():
    cmd = [f'sudo pkill -INT -e iokerneld ; \
            sudo pkill -INT -f synthetic ; \
            sudo pkill -INT -e dpdk-ctrl.elf ; \
           sudo pkill -INT -e phttp-bench ; \
           sudo pkill -INT -f tcpdump ; \
            sudo pkill -INT -e {SERVER_APP} ']
    # print(cmd)
    if TCPDUMP:
        cmd[0] += ' ; sudo pkill -INT -f -e tcpdump'
    # cmd = [f'sudo pkill -INT -f Capybara && sleep 2 && sudo pkill -f Capybara && sudo pkill -f caladan']
    kill_tasks = []
    for node in ALL_NODES:
        host = pyrem.host.RemoteHost(node)
        task = host.run(cmd, quiet=False)
        # print(task)
        kill_tasks.append(task)
    
    pyrem.task.Parallel(kill_tasks, aggregate=True).start(wait=True)
    print('KILLED CAPYBARA PROCESSES')


def run_server():
    global experiment_id
    
    print('SETUP SWITCH')
    cmd = [f'ssh sw1 "source /home/singtel/tools/set_sde.bash && \
           /home/singtel/bf-sde-9.4.0/run_bfshell.sh -b /home/singtel/inho/Capybara/capybara/p4/capybara_msr/capybara_msr_setup.py"'] 
    result = subprocess.run(
        cmd,
        shell=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=True,
    ).stdout.decode()
    # print(result + '\n\n')

    
    print('RUNNING BACKENDS')
    host = pyrem.host.RemoteHost(BACKEND_NODE)

    server_tasks = []
    for j in range(NUM_BACKENDS): 
        run_cmd = f'{AUTOKERNEL_PATH}/bin/examples/rust/{SERVER_APP}.elf 10.0.1.8:10008'
        
        
        
        if SERVER_APP == 'http-server' :
            cmd = [f'cd {AUTOKERNEL_PATH} && \
                sudo -E \
                CAPY_LOG={CAPY_LOG} \
                LIBOS={LIBOS} \
                MTU=1500 \
                MSS=1500 \
                RUST_BACKTRACE=full \
                {f"CONFIG_PATH={AUTOKERNEL_PATH}/scripts/config/node8_config.yaml"} \
                LD_LIBRARY_PATH={HOME}/lib:{HOME}/lib/x86_64-linux-gnu \
                PKG_CONFIG_PATH={HOME}/lib/x86_64-linux-gnu/pkgconfig \
                numactl -m0 \
                {run_cmd} \
                > {DATA_PATH}/{experiment_id}.be{j} 2>&1']
        else:
            print(f'Invalid server app: {SERVER_APP}')
            exit(1)
        task = host.run(cmd, quiet=False)
        server_tasks.append(task)
    pyrem.task.Parallel(server_tasks, aggregate=True).start(wait=False)    
    time.sleep(2)
    print(f'{NUM_BACKENDS} backends are running')

def run_tcpdump(experiment_id):
    print(f'RUNNING TCPDUMP to {TCPDUMP_NODE}:{PCAP_PATH}/{experiment_id}.pcap')
    
    host = pyrem.host.RemoteHost(TCPDUMP_NODE)
    cmd = [f'sudo tcpdump --time-stamp-precision=nano -i ens1f1 -w {PCAP_PATH}/{experiment_id}.pcap']
    task = host.run(cmd, quiet=False)
    pyrem.task.Parallel([task], aggregate=True).start(wait=False)
    
def parse_tcpdump(experiment_id):
    print(f'PARSING {TCPDUMP_NODE}:{PCAP_PATH}/{experiment_id}.pcap') 
    
    host = pyrem.host.RemoteHost(TCPDUMP_NODE)
    
    cmd = [f"""
            {AUTOKERNEL_PATH}/eval/pcap-parser.sh {PCAP_PATH}/{experiment_id}.pcap &&
            cat {PCAP_PATH}/{experiment_id}.csv | awk '{{if($16 == "GET"){{print $2}}}}' > {PCAP_PATH}/{experiment_id}.request_times &&
            cat {PCAP_PATH}/{experiment_id}.csv | awk '{{if($16 == "OK"){{print $2}}}}' > {PCAP_PATH}/{experiment_id}.response_times &&
            paste {PCAP_PATH}/{experiment_id}.request_times {PCAP_PATH}/{experiment_id}.response_times | awk '{{print $2-$1}}'  > {PCAP_PATH}/{experiment_id}.pcap_latency
    """]
    
    task = host.run(cmd, quiet=False)
    pyrem.task.Parallel([task], aggregate=True).start(wait=True)


def parse_latency_trace(experiment_id):
    print(f'PARSING {experiment_id} latency_trace') 
    
    cmd = f"cd {AUTOKERNEL_PATH}/eval\
            && sh parse_request_sched.sh {experiment_id}\
            && sh ms_avg_99p_lat.sh {experiment_id}\
            && sh ms_total_even_odd_numreq.sh {experiment_id}\
            && sh latency_cdf.sh {experiment_id}"
    print("Executing command:", cmd)  # For debugging

    result = subprocess.run(
        cmd,
        shell=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=True,
    ).stdout.decode()
    if result != '':
        print("ERROR: " + result + '\n\n')
    else:
        print("DONE")



def parse_wrk_result(experiment_id):
    import re
    result_str = ''
    # Read the text file
    with open(f'{DATA_PATH}/{experiment_id}.client', "r") as file:
        text = file.read()

    # Use regular expressions to extract relevant values
    avg_latency_match = re.search(r"(\d+\.\d+)(us|ms|s)\s", text)
    threads_match = re.search(r"(\d+)\s+threads", text)
    connections_match = re.search(r"(\d+)\s+connections", text)
    requests_sec_match = re.search(r"Requests/sec:\s+(\d+\.\d+)", text)
    # print(avg_latency_match)
    # Check if all necessary values were found
    if not (avg_latency_match and threads_match and connections_match and requests_sec_match):
        print("Failed to parse the text file")
    else:
        threads = threads_match.group(1)
        connections = connections_match.group(1)
        requests_per_sec = requests_sec_match.group(1)
         
        avg_lat = float(avg_latency_match.group(1)) * {'us': 1, 'ms': 1000, 's': 1000000}.get(avg_latency_match.group(2), 1)
        
        result_str = f'{experiment_id}, {NUM_BACKENDS}, {int(connections)}, {threads}, {requests_per_sec.split(".")[0]}, {int(avg_lat)}'

        # Define a regular expression pattern to match the percentages and values
        pattern = r'(\d+%)\s+(\d+\.\d+(?:us|ms|s))'
        # Find all matches in the text using the regular expression pattern
        matches = re.findall(pattern, text)

        # Create a dictionary to store the parsed data
        latency_data = {}

        # Iterate through the matches and populate the dictionary
        for match in matches:
            percentile, value = match
            latency_data[percentile] = value
            pattern = r"(\d+\.\d+)(us|ms|s)"
            matches = re.search(pattern, value)
            if matches:
                percentiile_lat = float(matches.group(1)) * {'us': 1, 'ms': 1000, 's': 1000000}.get(matches.group(2), 1)
            else:
                print("PANIC: cannot find percentile latency")
                exit(1)

            result_str = result_str + f', {int(percentiile_lat)}'
        # Print the parsed data
        # for percentile, value in latency_data.items():
        #     print(f"{percentile}: {value}")

        # # Create a CSV-style string
        # csv_data = f"{threads},{connections},{requests_per_sec}"

        # # Print the CSV-style string
        # print(csv_data)
    # print(result_str)
    return result_str
        

def parse_server_reply(experiment_id):
    print(f'PARSING {experiment_id} server reply') 
    cmd = f"cd {AUTOKERNEL_PATH}/eval && sh parse_server_reply.sh {experiment_id}"
    print("Executing command:", cmd)  # For debugging


    result = subprocess.run(
        cmd,
        shell=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=True,
    ).stdout.decode()
    if result != '':
        print("ERROR: " + result + '\n\n')
    else:
        print("DONE")
    return
    

def run_eval():
    global experiment_id
    global final_result
    
    if CLIENT_APP == 'caladan' and LOADSHIFTS.count('|') != 0 and LOADSHIFTS.count('|') != NUM_BACKENDS - 1:
        print(f"Error: LOADSHIFT configuration is wrong (check '|')")
        kill_procs()
        exit(1)
    for repeat in range(0, REPEAT_NUM):
        for pps in CLIENT_PPS:
            for conn in NUM_CONNECTIONS:
                for num_thread in NUM_THREADS:
                    kill_procs()
                    experiment_id = datetime.datetime.now().strftime('%Y%m%d-%H%M%S.%f')
                    
                    with open(f'{AUTOKERNEL_PATH}/eval/test_config.py', 'r') as file:
                        print(f'================ RUNNING TEST =================')
                        print(f'\n\nEXPTID: {experiment_id}')
                        with open(f'{DATA_PATH}/{experiment_id}.test_config', 'w') as output_file:
                            output_file.write(file.read())
                    if TCPDUMP == True:
                        run_tcpdump(experiment_id)
                    
                    run_server()

                    #exit(0)
                    host = pyrem.host.RemoteHost(CLIENT_NODE)
                    
                    cmd = [f'cd {CALADAN_PATH} && sudo ./iokerneld ias nicpci 0000:31:00.1']
                    task = host.run(cmd, quiet=True)
                    pyrem.task.Parallel([task], aggregate=True).start(wait=False)
                    time.sleep(3)
                    print('iokerneld is running')
                    
                    if SERVER_APP == 'http-server' :   
                        cmd = [f'sudo numactl -m0 {CALADAN_PATH}/apps/synthetic/target/release/synthetic \
                            10.0.1.8:10000 \
                            --config {CALADAN_PATH}/client.config \
                            --mode runtime-client \
                            --protocol=http \
                            --transport=tcp \
                            --samples=1 \
                            --pps={pps} \
                            --threads={conn} \
                            --runtime={RUNTIME} \
                            --discard_pct=10 \
                            --output=trace \
                            --rampup=0 \
                            {f"--loadshift={LOADSHIFTS}" if LOADSHIFTS != "" else ""} \
                            {f"--zipf={ZIPF_ALPHA}" if ZIPF_ALPHA != "" else ""} \
                            {f"--onoff={ONOFF}" if ONOFF == "1" else ""} \
                            --exptid={DATA_PATH}/{experiment_id} \
                            > {DATA_PATH}/{experiment_id}.client']
                    
                    else:
                        print(f'Invalid server app: {SERVER_APP}')
                        exit(1)
                    task = host.run(cmd, quiet=False)
                    pyrem.task.Parallel([task], aggregate=True).start(wait=True)

                    print('================ TEST COMPLETE =================\n')
                    if CLIENT_APP == 'wrk':
                        result_str = parse_wrk_result(experiment_id)
                        print(result_str + '\n\n')
                        final_result = final_result + result_str + '\n'
                    else:
                        try:
                            cmd = f'cat {DATA_PATH}/{experiment_id}.client | grep "\[RESULT\]" | tail -1'
                            result = subprocess.run(
                                cmd,
                                shell=True,
                                stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT,
                                check=True,
                            ).stdout.decode()
                            if result == '':
                                result = '[RESULT] N/A\n'
                            print('[RESULT]' + f'{experiment_id}, {conn}, {result[len("[RESULT]"):]}' + '\n\n')
                            final_result = final_result + f'{experiment_id}, {conn},{result[len("[RESULT]"):]}'
                        except subprocess.CalledProcessError as e:
                            # Handle the exception for a failed command execution
                            print("EXPERIMENT FAILED\n\n")

                        except Exception as e:
                            # Handle any other unexpected exceptions
                            print("EXPERIMENT FAILED\n\n")
                    
                    kill_procs()
                    time.sleep(7)
                    if TCPDUMP == True:
                        parse_tcpdump(experiment_id)
                        # print("Parsing pcap file is done, finishing test here.\n\n")

                        # exit()

                    
                    if EVAL_LATENCY_TRACE == True:
                        parse_latency_trace(experiment_id)

                    if EVAL_SERVER_REPLY == True:
                        parse_server_reply(experiment_id)


def exiting():
    global final_result
    print('EXITING')
    result_header = "ID, #CONN, Distribution, RPS, Target, Actual, Dropped, Never Sent, Median, 90th, 99th, 99.9th, 99.99th, Start, StartTsc"
    if CLIENT_APP == 'wrk':
        result_header = "ID, #BE, #CONN, #THREAD, AVG, p50, p75, p90, p99"
        
    print(f'\n\n\n\n\n{result_header}')
    print(final_result)
    with open(f'{DATA_PATH}/result.txt', "w") as file:
        file.write(f'{result_header}')
        file.write(final_result)
    kill_procs()


def run_compile():

    features = '--features=' if len(FEATURES) > 0 else ''
    for feat in FEATURES:
        features += feat + ','

    # if SERVER_APP == 'http-server':
    os.system(f"cd {AUTOKERNEL_PATH} && EXAMPLE_FEATURES={features} make LIBOS={LIBOS} all-examples-rust")
    if SERVER_APP == 'redis-server':
        clean = 'make clean-redis &&' if len(sys.argv) > 2 and sys.argv[2] == 'clean' else ''
        return os.system(f'cd {AUTOKERNEL_PATH} && {clean} EXAMPLE_FEATURES={features} REDIS_LOG={REDIS_LOG} make redis-server{mig}')
    # else:
    #     print(f'Invalid server app: {SERVER_APP}')
    #     exit(1)


if __name__ == '__main__':
    # parse_server_reply("20240228-065133.628398")
    # exit(1)
    # parse_result()
    # parse_mig_delay("20240318-093754.379579")
    # exit()

    if len(sys.argv) > 1 and sys.argv[1] == 'build':
        exit(run_compile())
    
    atexit.register(exiting)
    # cleaning()
    run_eval()
    # parse_result()
    kill_procs()