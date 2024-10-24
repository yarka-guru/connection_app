document.addEventListener('DOMContentLoaded', () => {
    const form = document.getElementById('connection-form');

    form.addEventListener('submit', async (event) => {
        event.preventDefault();

        const awsEnvironment = document.getElementById('aws-environment').value;
        const connectionDetails = document.getElementById('connection-details').value;

        try {
            const response = await fetch('/connect', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({ awsEnvironment, connectionDetails })
            });

            if (!response.ok) {
                throw new Error('Network response was not ok');
            }

            const data = await response.json();
            displayConnectionDetails(data);
        } catch (error) {
            console.error('Error:', error);
        }
    });

    function displayConnectionDetails(details) {
        const detailsContainer = document.createElement('div');
        detailsContainer.innerHTML = `
            <h2>Connection Details</h2>
            <p>Host: ${details.host}</p>
            <p>Port: ${details.port}</p>
            <p>User: ${details.user}</p>
            <p>Database: ${details.database}</p>
            <p>Password: ${details.password}</p>
        `;
        document.body.appendChild(detailsContainer);
    }
});
