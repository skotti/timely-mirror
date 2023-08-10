#!/usr/bin/env python3
# %%
import os
import re
import csv

def extract_info_from_file(file_path):
    with open(file_path, 'r') as f:
        content = f.read()

    epoch_throughput = re.search(r'epoch throughput: ([\d.]+)', content).group(1)
    
    latency_percentiles = re.findall(r'0.5, (\d+), (\d+)', content)
    if latency_percentiles:
        lower_bound, upper_bound = latency_percentiles[0]
        epoch_latency_0_5 = str((int(lower_bound) + int(upper_bound)) / 2)
    else:
        epoch_latency_0_5 = 'N/A'
    
    return epoch_throughput, epoch_latency_0_5

output_dir = 'data-v1/'
output_files = [f for f in os.listdir(output_dir) if f.startswith('output_')]

output_data = []

for file_name in output_files:
    index = int(re.search(r'output_(\d+)\.txt', file_name).group(1))
    file_path = os.path.join(output_dir, file_name)
    epoch_throughput, epoch_latency_0_5 = extract_info_from_file(file_path)
    output_data.append((index, epoch_throughput, epoch_latency_0_5))

csv_file = 'output_data.csv'

with open(csv_file, 'w', newline='') as f:
    writer = csv.writer(f)
    writer.writerow(['index', 'epoch_throughput', 'epoch_latency_0_5'])
    writer.writerows(output_data)

print(f"Data extracted and saved to '{csv_file}'")

# %%
