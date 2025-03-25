# Air Quality Monitoring Dashboard

This Streamlit application provides a real-time monitoring dashboard for air quality sensors connected to an ESP8266 microcontroller. The application fetches data through a Python adapter that communicates with the ESP8266 device, visualizes current and historical readings, and provides interpretations and predictions for the environmental conditions.

## Features

- Real-time monitoring of temperature, humidity, and air quality
- Data fetching through a Python adapter between ESP8266 and the dashboard
- Historical data visualization with interactive charts
- Automatic data refresh at configurable intervals
- Basic prediction of future sensor values using linear regression
- Human-readable interpretations of sensor readings
- Responsive design that works on various devices
- Demo mode for testing without an actual ESP8266 device

## Requirements

- Python 3.8 or higher
- Streamlit
- Pandas
- NumPy
- scikit-learn
- Requests

## Setup and Installation

1. Ensure your ESP8266 device is running the provided Arduino code (`air_monitoring_system.ino`) and is connected to the same network as your computer.

2. Install the required Python packages:
   ```
   pip install streamlit pandas numpy scikit-learn requests
   ```

3. Run the Streamlit application:
   ```
   streamlit run app.py
   ```

4. The dashboard should open in your web browser automatically. If not, navigate to the URL shown in the terminal (typically http://localhost:8051).

## Configuration

You can customize the dashboard through the sidebar controls:

- **ESP8266 IP Address**: Update if your device has a different IP address
- **Demo Mode**: Enable to generate sample data when no ESP8266 device is available
- **Update Frequency**: Control how often the dashboard fetches new data
- **Data History**: Set how many data points to keep in the history
- **Prediction Points**: Control how many future points to predict

## Understanding the Data

### Temperature
- Measures the ambient temperature in degrees Celsius (°C)
- Comfortable range is typically between 18°C and 24°C
- Visualized with temperature-appropriate color coding

### Humidity
- Measures the relative humidity as a percentage (%)
- Comfortable range is typically between 30% and 50%
- Important for comfort and prevention of mold/dryness issues

### Air Quality
- Measures air quality in parts per million (PPM)
- Lower values indicate cleaner air
- Values below 100 PPM generally indicate excellent air quality
- Higher values may indicate presence of pollutants requiring ventilation

## Troubleshooting

If you encounter issues connecting to the ESP8266:

1. Verify that the ESP8266 is powered on and connected to the same network as your computer
2. Check that the IP address in the dashboard sidebar matches your ESP8266's IP address
3. Ensure there are no firewall settings blocking the connection
4. Try enabling "Demo Mode" to see the dashboard functionality without requiring the actual device
5. Restart the ESP8266 device if necessary

## How It Works

The application continuously polls the ESP8266 device, which runs a web server that returns JSON data from the connected sensors. The dashboard processes this data, stores it in a session state for historical tracking, and uses simple linear regression to make predictions about future values.

In demo mode, the application generates realistic sensor data that mimics the behavior of actual sensors, allowing you to test and demonstrate the dashboard functionality without requiring the physical hardware.

## References

This application adapts code from the original `esp.py` Python script and integrates with the `air_monitoring_system.ino` Arduino program running on the ESP8266 device. The demo mode functionality was added to allow viewing and testing of the dashboard without requiring the physical ESP8266 hardware.
